use super::*;

fn permanent_deletion_state(state: slipstream_core::PermanentDeletionItemState) -> &'static str {
    match state {
        slipstream_core::PermanentDeletionItemState::Pending => "pending",
        slipstream_core::PermanentDeletionItemState::Deleting => "deleting",
        slipstream_core::PermanentDeletionItemState::Deleted => "deleted",
        slipstream_core::PermanentDeletionItemState::Missing => "missing",
        slipstream_core::PermanentDeletionItemState::Changed => "changed",
        slipstream_core::PermanentDeletionItemState::Failed => "failed",
        slipstream_core::PermanentDeletionItemState::Uncertain => "uncertain",
    }
}

fn permanent_deletion_response(
    result: slipstream_core::PermanentDeletionResult,
) -> PermanentDeletionResponse {
    PermanentDeletionResponse {
        operation_id: result.operation_id,
        reviewed: result.reviewed,
        logical_bytes_deleted: result.logical_bytes_deleted,
        items: result
            .items
            .into_iter()
            .map(|item| PermanentDeletionItemWire {
                photo_id: item.photo_id,
                original_location: item.relative_path.to_string(),
                original_kind: original_kind(&item.kind),
                state: permanent_deletion_state(item.state),
                size: item.size,
                message: item.message,
            })
            .collect(),
    }
}
fn explicit_photo_removal_response(
    result: slipstream_core::PhotoRemovalResult,
) -> ExplicitPhotoRemovalResponse {
    let slipstream_core::PhotoRemovalResult {
        operation_id,
        counts,
        ordered_photo_ids,
        removed,
        removed_markers,
        changed_elsewhere,
        missing,
        already_removed,
        ..
    } = result;
    let markers = removed_markers
        .into_iter()
        .map(|marker| (marker.photo_id, marker.removed_at_ms))
        .collect::<std::collections::HashMap<_, _>>();
    let removed = removed
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let changed_elsewhere = changed_elsewhere
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let missing = missing
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let already_removed = already_removed
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let results = ordered_photo_ids
        .into_iter()
        .map(|photo_id| {
            let (outcome, removed_at_ms) = if removed.contains(photo_id.as_str()) {
                ("removed", markers.get(&photo_id).copied())
            } else if changed_elsewhere.contains(photo_id.as_str()) {
                ("changed-elsewhere", None)
            } else if missing.contains(photo_id.as_str()) {
                ("unavailable", None)
            } else if already_removed.contains(photo_id.as_str()) {
                ("already-removed", None)
            } else {
                unreachable!("removal result omitted a requested Photo")
            };
            PhotoRemovalItemWire {
                photo_id,
                outcome,
                removed_at_ms,
            }
        })
        .collect();
    ExplicitPhotoRemovalResponse {
        operation_id,
        counts: PhotoRemovalCountsWire {
            removed: counts.removed,
            changed_elsewhere: counts.changed_elsewhere,
            missing: counts.missing,
            already_removed: counts.already_removed,
        },
        results,
    }
}

fn explicit_photo_restore_response(
    result: slipstream_core::ExplicitPhotoRestoreResult,
) -> ExplicitPhotoRestoreResponse {
    let slipstream_core::ExplicitPhotoRestoreResult {
        operation_id,
        ordered_photo_ids,
        counts,
        restored,
        already_active,
        changed_elsewhere,
        missing,
    } = result;
    let restored = restored
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let already_active = already_active
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let changed_elsewhere = changed_elsewhere
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let missing = missing
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let results = ordered_photo_ids
        .into_iter()
        .map(|photo_id| {
            let outcome = if restored.contains(photo_id.as_str()) {
                "restored"
            } else if already_active.contains(photo_id.as_str()) {
                "already-active"
            } else if changed_elsewhere.contains(photo_id.as_str()) {
                "changed-elsewhere"
            } else if missing.contains(photo_id.as_str()) {
                "unavailable"
            } else {
                unreachable!("restore result omitted a requested Photo")
            };
            PhotoRestoreItemWire { photo_id, outcome }
        })
        .collect();
    ExplicitPhotoRestoreResponse {
        operation_id,
        counts: ExplicitPhotoRestoreCountsWire {
            restored: counts.restored,
            already_active: counts.already_active,
            changed_elsewhere: counts.changed_elsewhere,
            missing: counts.missing,
        },
        results,
    }
}

impl Application {
    /// Confirms the removal of one reviewed rejected result.
    ///
    /// The Browse Snapshot supplies the complete reviewed result and the
    /// Selection State filter it was reviewed under, so removal can only ever
    /// hide Photos the browser actually showed as Rejected. The browser
    /// supplies the operation id, so a retried request repeats one operation
    /// instead of creating a second one, and Undo names the group by that id
    /// rather than by a transferred Photo list.
    pub async fn remove_photos(
        &self,
        token: &str,
        operation_id: &str,
    ) -> Result<PhotoRemovalResponse, ServerError> {
        let (photo_ids, filter) = self
            .retained_queries
            .browse_snapshot(token, Instant::now())
            .ok_or(ServerError::BrowseNotFound)?;
        if filter != Some(BrowseSelectionFilter::Rejected) {
            return Err(ServerError::RemovalFilter);
        }
        // The write and the publication patch are one critical section: a
        // reader that serializes on `publication` cannot acquire it between
        // the commit and the patch and observe the Photos as still present.
        let _publication = self.shared.publication.lock().await;
        let result = self
            .library
            .remove_photos(slipstream_core::PhotoRemovalMutation {
                photo_ids,
                operation_id: operation_id.to_owned(),
            })
            .await?;
        self.shared
            .patch_photo_removals(&result.newly_removed, true);
        Ok(PhotoRemovalResponse {
            operation_id: result.operation_id,
            counts: PhotoRemovalCountsWire {
                removed: result.counts.removed,
                changed_elsewhere: result.counts.changed_elsewhere,
                missing: result.counts.missing,
                already_removed: result.counts.already_removed,
            },
            changed_elsewhere: result.changed_elsewhere,
            missing: result.missing,
            already_removed: result.already_removed,
        })
    }
    /// Applies one explicit removal set after rechecking every submitted
    /// decision and removal-state evidence.
    pub async fn remove_photos_explicit(
        &self,
        mutation: slipstream_core::ExplicitPhotoRemovalMutation,
    ) -> Result<ExplicitPhotoRemovalResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let result = self.library.remove_photos_explicit(mutation).await?;
        self.shared
            .patch_photo_removals(&result.newly_removed, true);
        Ok(explicit_photo_removal_response(result))
    }

    /// Reads an accepted explicit removal result without changing state.
    pub async fn photo_removal_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRemovalResponse>, ServerError> {
        Ok(self
            .library
            .photo_removal_operation(operation_id)
            .await?
            .map(explicit_photo_removal_response))
    }

    /// Applies one explicit Restore attempt and retains its result in the
    /// persistence owner for replay and inspection.
    pub async fn restore_photos_explicit(
        &self,
        mutation: slipstream_core::ExplicitPhotoRestoreMutation,
    ) -> Result<ExplicitPhotoRestoreResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let replay = self
            .library
            .photo_restore_operation(mutation.operation_id.clone())
            .await?
            .is_some();
        let result = self.library.restore_photos_explicit(mutation).await?;
        if !replay {
            self.shared.patch_photo_removals(&result.restored, false);
        }
        Ok(explicit_photo_restore_response(result))
    }

    /// Applies one fixed admission-time Trash selection.
    pub async fn restore_trash(
        &self,
        operation_id: String,
        selection: slipstream_core::PermanentDeletionSelection,
    ) -> Result<ExplicitPhotoRestoreResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let replay = self
            .library
            .photo_restore_operation(operation_id.clone())
            .await?
            .is_some();
        let result = self.library.restore_trash(operation_id, selection).await?;
        if !replay {
            self.shared.patch_photo_removals(&result.restored, false);
        }
        Ok(explicit_photo_restore_response(result))
    }

    /// Reads an accepted explicit Restore result without changing state.
    pub async fn photo_restore_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRestoreResponse>, ServerError> {
        Ok(self
            .library
            .photo_restore_operation(operation_id)
            .await?
            .map(explicit_photo_restore_response))
    }

    /// Restores every Photo one operation still owns, or one explicit bounded
    /// set, and patches the removed fact back into the published Library.
    pub async fn restore_photos(
        &self,
        restoration: slipstream_core::PhotoRestoration,
    ) -> Result<PhotoRestorationResponse, ServerError> {
        // As for a removal, the write and its published effect are one
        // critical section: a restored Photo cannot be missing from a source
        // opened after the restore committed.
        let _publication = self.shared.publication.lock().await;
        let result = self.library.restore_photos(restoration).await?;
        self.shared.patch_photo_removals(&result.restored, false);
        Ok(PhotoRestorationResponse {
            counts: PhotoRestorationCountsWire {
                restored: result.counts.restored,
                changed_elsewhere: result.counts.changed_elsewhere,
                missing: result.counts.missing,
            },
            changed_elsewhere: result.changed_elsewhere,
            missing: result.missing,
            operations: result
                .operations
                .into_iter()
                .map(|remainder| PhotoOperationRemainderWire {
                    operation_id: remainder.operation_id,
                    removed: remainder.removed,
                })
                .collect(),
        })
    }

    /// One bounded page of removed Photos, newest removal first. The persisted
    /// removal order is authoritative; the published Library supplies the same
    /// Grid facts and current derivative URLs Grid View renders, so a removed
    /// Photo is recognizable without leaving the removal view.
    pub async fn removed_photos(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<RemovedPhotosResponse, ServerError> {
        if limit == 0 || limit > MAX_REMOVED_WINDOW {
            return Err(ServerError::RemovedWindow);
        }
        // The persisted page and the published facts are read as one
        // publication, so a removal or restore committed between them cannot
        // present a row whose removal the Library no longer holds.
        let _publication = self.shared.publication.lock().await;
        let (records, total, operation) = self.library.removed_photos(start, limit).await?;
        let facts = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let Some(published) = guard.as_ref() else {
                return Err(ServerError::NotPublished);
            };
            records
                .into_iter()
                .filter_map(|record| {
                    let position = published.photos_by_id.get(&record.photo_id).copied()?;
                    let photo = published.snapshot.photos.get(position)?;
                    let originals = [Some(&photo.original_id)]
                        .into_iter()
                        .flatten()
                        .filter_map(|id| published.originals_by_id.get(id))
                        .filter_map(|position| published.snapshot.originals.get(*position))
                        .cloned()
                        .collect();
                    Some((
                        record.removed_at_ms,
                        record.pending_verification,
                        PreviewFacts::from_records(photo.clone(), originals),
                    ))
                })
                .collect::<Vec<_>>()
        };
        let mut photos = Vec::with_capacity(facts.len());
        for (removed_at_ms, pending_verification_operation_id, facts) in facts {
            let Some(original) = facts
                .originals
                .iter()
                .find(|original| original.id == facts.photo.original_id)
            else {
                continue;
            };
            let original_kind = original_kind(&original.kind);
            let (preview_url, thumbnail_url) = self.derivative_urls(&facts).await;
            let originals_by_id = facts
                .originals
                .iter()
                .enumerate()
                .map(|(position, original)| (original.id.clone(), position))
                .collect::<std::collections::HashMap<_, _>>();
            photos.push(RemovedPhotoWire {
                removed_at_ms,
                original_location: original.relative_path.to_string(),
                original_kind,
                original_size: original.available.then_some(original.facts.size),
                pending_verification_operation_id,
                photo: photo_summary_indexed_with_url(
                    &facts.photo,
                    &facts.originals,
                    &originals_by_id,
                    preview_url,
                    thumbnail_url,
                ),
            });
        }
        Ok(RemovedPhotosResponse {
            start,
            limit,
            total,
            review_maximum: slipstream_core::PERMANENT_DELETION_MAX,
            operation: operation.map(|operation| PhotoOperationRemainderWire {
                operation_id: operation.operation_id,
                removed: operation.removed,
            }),
            photos,
        })
    }
    pub async fn prepare_permanent_deletion(
        &self,
        operation_id: String,
        selection: slipstream_core::PermanentDeletionSelection,
    ) -> Result<PermanentDeletionReviewResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let review = self
            .library
            .prepare_permanent_deletion(operation_id, selection)
            .await?;
        Ok(PermanentDeletionReviewResponse {
            operation_id: review.operation_id,
            items: review
                .items
                .into_iter()
                .map(|item| PermanentDeletionReviewItemWire {
                    photo_id: item.photo_id,
                    removed_at_ms: item.removed_at_ms,
                    original_id: item.original_id,
                    original_location: item.relative_path.to_string(),
                    original_kind: original_kind(&item.kind).to_owned(),
                    size: item.size,
                    albums: item
                        .albums
                        .into_iter()
                        .map(|album| PhotoAlbumMembershipWire {
                            id: album.album_id,
                            name: album.album_name,
                        })
                        .collect(),
                })
                .collect(),
            rejected: review
                .rejected
                .into_iter()
                .map(|(photo_id, reason)| PermanentDeletionRejectionWire {
                    photo_id,
                    reason: match reason {
                        slipstream_core::PermanentDeletionRejection::Missing => "missing",
                        slipstream_core::PermanentDeletionRejection::ChangedElsewhere => {
                            "changed-elsewhere"
                        }
                        slipstream_core::PermanentDeletionRejection::PendingVerification => {
                            "pending-verification"
                        }
                    },
                })
                .collect(),
        })
    }

    pub async fn permanently_delete(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let result = self.library.permanently_delete(operation_id).await?;
        let deleted_photo_ids = result
            .items
            .iter()
            .filter(|item| item.state == slipstream_core::PermanentDeletionItemState::Deleted)
            .map(|item| item.photo_id.clone())
            .collect::<Vec<_>>();
        self.shared.patch_permanently_deleted(&deleted_photo_ids);
        Ok(permanent_deletion_response(result))
    }

    pub async fn read_permanent_deletion(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        Ok(permanent_deletion_response(
            self.library.read_permanent_deletion(operation_id).await?,
        ))
    }
}
