use super::*;

impl Library {
    pub async fn replay_composable_edit_recipe(
        &self,
        mutation: SaveComposableEditRecipe,
    ) -> Result<Option<ComposableEditRecipeWriteOutcome>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .replay_composable_edit_recipe_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn replay_processing_export_retry(
        &self,
        photo_id: &str,
        previous_request_id: &str,
        request_id: &str,
    ) -> Result<Option<crate::processing::ProcessingExportSubmitOutcome>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.replay_processing_export_retry_receiver(
                photo_id,
                previous_request_id,
                request_id,
            )
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn composable_edit_recipe_read(
        &self,
        photo_id: &str,
    ) -> Result<Option<crate::processing::ComposableEditRecipeRead>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .composable_edit_recipe_read_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn rebind_composable_edit_recipe(
        &self,
        mutation: crate::processing::RebindComposableEditRecipe,
    ) -> Result<ComposableEditRecipeWriteOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .rebind_composable_edit_recipe_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn list_processing_exports(
        &self,
        photo_id: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportList, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .list_processing_exports_receiver(photo_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn replay_processing_export(
        &self,
        mutation: crate::processing::ReplayProcessingExport,
    ) -> Result<Option<crate::processing::ProcessingExportSubmitOutcome>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.replay_processing_export_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn retry_processing_export(
        &self,
        mutation: crate::processing::RetryProcessingExport,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportSubmitOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .retry_processing_export_receiver(mutation, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn processing_artifact_retention(
        &self,
        artifact_id: &str,
    ) -> Result<Option<crate::processing::ProcessingArtifactRetention>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .read_processing_artifact_retention_receiver(artifact_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads one Photo's retained composable recipe.
    pub async fn composable_edit_recipe(
        &self,
        photo_id: &str,
    ) -> Result<Option<ComposableEditRecipe>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.composable_edit_recipe_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Saves one complete composable Edit Recipe only when both the caller's
    /// expected recipe revision and observed Library source revision still
    /// match current persistence, decided inside one serialized
    /// persistence-owner operation.
    pub async fn save_composable_edit_recipe(
        &self,
        mutation: SaveComposableEditRecipe,
    ) -> Result<ComposableEditRecipeWriteOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .save_composable_edit_recipe_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads one published immutable Processing Artifact in a single
    /// serialized persistence-owner operation. `None` means no artifact was
    /// ever published under the identity; the legacy Export surface is a
    /// separate fixed store that is not consulted.
    pub async fn processing_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<Option<crate::processing::ProcessingArtifact>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .read_processing_artifact_receiver(artifact_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Publishes one validated Processing Artifact insert-only inside one
    /// serialized persistence-owner operation: a fresh identity installs,
    /// an identical record replays, and any different record under the
    /// same identity is refused unchanged.
    pub async fn publish_processing_artifact(
        &self,
        artifact: crate::processing::ProcessingArtifact,
    ) -> Result<crate::processing::ProcessingArtifactPublication, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .publish_processing_artifact_receiver(artifact)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Submits the selected current Processing Step of a Photo for an
    /// explicit Export, decided inside one serialized persistence-owner
    /// operation. Admission captures the stored step's exact identity,
    /// validates the concrete input handoff, and either records the
    /// deployment's explicit adapter refusal, replays a recorded decision,
    /// or admits a qualified execution — committing its accepted work
    /// record in the same transaction, so a duplicate live request resolves
    /// to `Pending` and never starts a second execution. Nothing is
    /// executed here.
    pub async fn submit_processing_export(
        &self,
        mutation: crate::processing::SubmitProcessingExport,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportSubmitOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .submit_processing_export_receiver(mutation, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Settles one admitted qualified composable Export inside one
    /// serialized persistence-owner operation: the validated artifact is
    /// published insert-only, the work is recorded `Succeeded`, and the
    /// request's acceptance receipt and the artifact's finite retention are
    /// installed in the same transaction, so a committed settlement always
    /// replays its immutable artifact under the same request identity and
    /// the first terminal decision wins.
    pub async fn settle_processing_export(
        &self,
        artifact: crate::processing::ProcessingArtifact,
        request_id: &str,
        payload_digest: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportSettlement, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.settle_processing_export_receiver(
                artifact,
                request_id,
                payload_digest,
                now,
            )
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads the durable work record of one admitted composable Export.
    /// `None` means no qualified submission was ever accepted under the
    /// request identity.
    pub async fn processing_export_work(
        &self,
        request_id: &str,
    ) -> Result<Option<crate::processing::ProcessingExportWork>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .read_processing_export_work_receiver(request_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Durably begins one execution attempt of an accepted composable
    /// Export inside one serialized persistence-owner operation. The first
    /// terminal decision wins over any later lifecycle change.
    pub async fn begin_processing_export_attempt(
        &self,
        request_id: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportAttemptOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .begin_processing_export_attempt_receiver(request_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Records one terminal execution failure of an admitted composable
    /// Export inside one serialized persistence-owner operation. The first
    /// terminal decision wins over any later lifecycle change.
    pub async fn fail_processing_export(
        &self,
        request_id: &str,
        reason_code: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportFailureOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .fail_processing_export_receiver(request_id, reason_code, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Cancels one admitted composable Export inside one serialized
    /// persistence-owner operation. The first terminal decision wins over
    /// any later lifecycle change.
    pub async fn cancel_processing_export(
        &self,
        request_id: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingExportCancelOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .cancel_processing_export_receiver(request_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Every live composable Export work record — accepted or executing —
    /// for restart reconciliation.
    pub async fn unfinished_processing_exports(
        &self,
    ) -> Result<Vec<crate::processing::ProcessingExportWork>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.unfinished_processing_exports_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Removes expired Processing Artifacts, their retention, and their
    /// terminal work inside one serialized persistence-owner operation, and
    /// tombstones their request identities. Returns the expired artifact
    /// identities whose files the caller deletes after the commit.
    pub async fn sweep_processing_export_expiry(
        &self,
        now: u64,
    ) -> Result<Vec<String>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .sweep_processing_export_expiry_receiver(now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Acquires one Processing Artifact download lease inside one
    /// serialized persistence-owner operation: the lease holds the artifact
    /// against expiry cleanup while it is renewed.
    pub async fn acquire_processing_artifact_lease(
        &self,
        artifact_id: &str,
        now: u64,
    ) -> Result<crate::processing::ProcessingArtifactLeaseOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .acquire_processing_artifact_lease_receiver(artifact_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Refreshes one Processing Artifact download lease's liveness anchor;
    /// `false` means the lease is gone and the stream must stop renewing.
    pub async fn renew_processing_artifact_lease(
        &self,
        lease_id: &str,
        now: u64,
    ) -> Result<bool, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .renew_processing_artifact_lease_receiver(lease_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Releases one Processing Artifact download lease once its stream has
    /// settled; `false` means the lease was already gone.
    pub async fn release_processing_artifact_lease(
        &self,
        lease_id: &str,
    ) -> Result<bool, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .release_processing_artifact_lease_receiver(lease_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }
}
