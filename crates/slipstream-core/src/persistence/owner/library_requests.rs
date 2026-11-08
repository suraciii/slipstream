//! Album, Photo, and Photo Decision request surface of the persistence owner.
//!
//! Each builder validates or normalizes its request, submits one command on the
//! owner's ordered queue, and returns the reply receiver. The owner runtime,
//! command dispatch, and lifecycle remain in `owner`.

use super::*;

impl Persistence {
    pub async fn list_albums(&self) -> Result<Vec<AlbumRecord>, PersistenceError> {
        let receive = self.list_albums_receiver()?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn list_albums_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<AlbumRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(Command::ListAlbums)
    }

    /// Bounded summaries for browser routes: counts and saved-position
    /// existence stay in SQL; no member rows are materialized.
    pub(crate) fn list_album_summaries_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<AlbumSummary>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(Command::ListAlbumSummaries)
    }

    pub(crate) fn album_receiver(
        &self,
        album_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<AlbumSummary>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::ReadAlbum {
            album_id: album_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn albums_by_id_receiver(
        &self,
        album_ids: Vec<String>,
    ) -> Result<AlbumReadWindowReceiver, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::ReadAlbums { album_ids, reply })
    }

    pub(crate) fn create_album_query_receiver(
        &self,
        filter: AlbumQueryFilter,
        maximum_results: usize,
    ) -> Result<oneshot::Receiver<Result<Vec<String>, PhotoQueryError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateAlbumQuery {
            filter,
            maximum_results,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn photo_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<PhotoRead>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::ReadPhoto {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn edit_recipe_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<EditRecipeRead>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::ReadEditRecipe {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn edit_recipe_surface_receiver(
        &self,
        photo_id: &str,
    ) -> Result<EditRecipeSurfaceReceiver, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::ReadEditRecipeSurface {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn read_development_proxy_receiver(
        &self,
        photo_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<crate::DevelopmentProxyRecord>, PersistenceError>>,
        PersistenceError,
    > {
        self.submit_persistence_receiver(|reply| Command::ReadDevelopmentProxy {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn record_development_proxy_receiver(
        &self,
        record: crate::DevelopmentProxyRecord,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::RecordDevelopmentProxy(record, reply))
    }

    pub(crate) fn remove_development_proxy_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::RemoveDevelopmentProxy {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    pub(crate) fn all_development_proxies_receiver(
        &self,
    ) -> Result<
        oneshot::Receiver<Result<Vec<crate::DevelopmentProxyRecord>, PersistenceError>>,
        PersistenceError,
    > {
        self.submit_persistence_receiver(Command::AllDevelopmentProxies)
    }

    pub(crate) fn photos_by_id_receiver(
        &self,
        photo_ids: Vec<String>,
        projection: Arc<PhotoQueryProjection>,
    ) -> Result<PhotoReadWindowReceiver, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::ReadPhotos {
            photo_ids,
            projection,
            reply,
        })
    }

    pub(crate) fn create_photo_query_receiver(
        &self,
        query: PhotoQuery,
        projection: Arc<PhotoQueryProjection>,
        maximum_results: usize,
    ) -> Result<oneshot::Receiver<Result<Vec<String>, PhotoQueryError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreatePhotoQuery {
            query,
            projection,
            maximum_results,
            reply: send,
        })?;
        Ok(receive)
    }

    /// Bounded per-Photo membership: the Albums containing one Photo, in
    /// Album-list order, without materializing any member list.
    pub(crate) fn photo_albums_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<PhotoAlbums>, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::PhotoAlbums {
            photo_id: photo_id.to_owned(),
            reply,
        })
    }

    /// Ordered membership identity for one Album's Browse Snapshot.
    pub(crate) fn album_browse_target_receiver(
        &self,
        album_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<AlbumBrowseTarget>, PersistenceError>>,
        PersistenceError,
    > {
        self.submit_persistence_receiver(|reply| Command::AlbumBrowseTarget {
            album_id: album_id.to_owned(),
            reply,
        })
    }

    pub async fn mutate_album(
        &self,
        mutation: AlbumMutation,
    ) -> Result<AlbumMutationResult, MutationError> {
        let receive = self.mutate_album_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_album_receiver(
        &self,
        mutation: AlbumMutation,
    ) -> Result<oneshot::Receiver<Result<AlbumMutationResult, MutationError>>, MutationError> {
        let mutation = normalize_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbum(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_album_membership(
        &self,
        mutation: AlbumMembershipMutation,
    ) -> Result<AlbumMembershipResult, MutationError> {
        let receive = self.mutate_album_membership_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_album_membership_receiver(
        &self,
        mutation: AlbumMembershipMutation,
    ) -> Result<oneshot::Receiver<Result<AlbumMembershipResult, MutationError>>, MutationError>
    {
        let mutation = normalize_album_membership_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumMembership(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn create_album_checked(
        &self,
        name: String,
    ) -> Result<AlbumCreationResult, AlbumWriteError> {
        let receive = self.create_album_checked_receiver(name)?;
        receive.await.unwrap_or(Err(AlbumWriteError::Persistence))
    }

    pub(crate) fn create_album_checked_receiver(
        &self,
        name: String,
    ) -> Result<oneshot::Receiver<Result<AlbumCreationResult, AlbumWriteError>>, AlbumWriteError>
    {
        let name = normalize_album_name(name)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateAlbum(name, send))
            .map_err(album_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_album_checked(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<CheckedAlbumMutationResult, AlbumWriteError> {
        let receive = self.mutate_album_checked_receiver(mutation)?;
        receive.await.unwrap_or(Err(AlbumWriteError::Persistence))
    }

    pub(crate) fn mutate_album_checked_receiver(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<
        oneshot::Receiver<Result<CheckedAlbumMutationResult, AlbumWriteError>>,
        AlbumWriteError,
    > {
        let mutation = normalize_checked_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumChecked(mutation, send))
            .map_err(album_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_photo_decision_checked(
        &self,
        mutation: CheckedPhotoDecisionMutation,
    ) -> Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError> {
        let receive = self.mutate_photo_decision_checked_receiver(mutation)?;
        receive
            .await
            .unwrap_or(Err(PhotoDecisionWriteError::Persistence))
    }

    pub(crate) fn mutate_photo_decision_checked_receiver(
        &self,
        mutation: CheckedPhotoDecisionMutation,
    ) -> Result<
        oneshot::Receiver<Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError>>,
        PhotoDecisionWriteError,
    > {
        let mutation = normalize_checked_photo_decision_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoDecisionChecked(mutation, send))
            .map_err(photo_decision_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: PhotoStateMutation,
    ) -> Result<PhotoStateMutationResult, MutationError> {
        let receive = self.mutate_photo_state_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_photo_state_receiver(
        &self,
        mutation: PhotoStateMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoStateMutationResult, MutationError>>, MutationError>
    {
        validate_photo_state_mutation(&mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoState(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn mutate_photo_state_batch_receiver(
        &self,
        mutation: PhotoStateBatchMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoStateBatchResult, MutationError>>, MutationError>
    {
        validate_photo_state_batch_mutation(&mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoStateBatch(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
}
