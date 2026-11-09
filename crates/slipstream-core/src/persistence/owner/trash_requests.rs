//! Trash request surface of the persistence owner.
//!
//! The owner's Trash commands share target admission: a bounded set of
//! identified removed Photos with unique Photo identities. Each builder submits
//! one command on the owner's ordered queue and returns the reply receiver.

use super::*;

fn require_trash_targets(targets: &[TrashCleanupTarget]) -> Result<(), MutationError> {
    if targets.is_empty()
        || targets.len() > crate::TRASH_CLEANUP_MAX
        || targets.iter().any(|target| {
            target.photo_id.is_empty() || target.original_id.is_empty() || target.removed_at_ms < 0
        })
        || targets
            .iter()
            .map(|target| &target.photo_id)
            .collect::<HashSet<_>>()
            .len()
            != targets.len()
    {
        return Err(MutationError::Invalid);
    }
    Ok(())
}

impl Persistence {
    pub(crate) fn trash_inspection_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<TrashInspectionRecord>, MutationError>>, MutationError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::TrashInspection(send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn cleanup_trash_receiver(
        &self,
        root: LibraryRoot,
        operation_id: String,
        targets: Vec<TrashCleanupTarget>,
    ) -> Result<oneshot::Receiver<Result<TrashCleanupResult, MutationError>>, MutationError> {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        require_trash_targets(&targets)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::CleanupTrash {
            root,
            operation_id,
            targets,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn preview_trash_cleanup_receiver(
        &self,
        root: LibraryRoot,
        targets: Vec<TrashCleanupTarget>,
    ) -> Result<oneshot::Receiver<Result<TrashCleanupPreview, MutationError>>, MutationError> {
        require_trash_targets(&targets)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::PreviewTrashCleanup {
            root,
            targets,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn read_trash_cleanup_receiver(
        &self,
        operation_id: String,
    ) -> Result<oneshot::Receiver<Result<TrashCleanupResult, MutationError>>, MutationError> {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadTrashCleanup {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
}
