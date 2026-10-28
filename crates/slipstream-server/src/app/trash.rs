use super::*;
use slipstream_core::{
    TrashCleanupItemState, TrashCleanupPreview, TrashCleanupResult, TrashCleanupTarget,
    TrashInspectionRecord,
};

fn preview_response(result: TrashCleanupPreview) -> TrashCleanupPreviewResponse {
    TrashCleanupPreviewResponse {
        items: result
            .items
            .into_iter()
            .map(|item| TrashCleanupPreviewItemWire {
                photo_id: item.photo_id,
                original_location: item.relative_path.to_string(),
                original_kind: original_kind(&item.kind),
                state: match item.state {
                    TrashCleanupItemState::Changed => "changed",
                    TrashCleanupItemState::Unavailable
                    | TrashCleanupItemState::InspectionFailed => "unavailable",
                    _ => "captured",
                },
                absence: match item.absence {
                    slipstream_core::TrashAbsence::Absent => "absent",
                    slipstream_core::TrashAbsence::Present => "present",
                    slipstream_core::TrashAbsence::Uninspectable => "uninspectable",
                },
                message: item.message,
                blockers: item
                    .blockers
                    .into_iter()
                    .map(|blocker| TrashCleanupBlockerWire {
                        kind: blocker.kind,
                        name: blocker.name,
                        action: blocker.action,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn cleanup_response(result: TrashCleanupResult) -> TrashCleanupResponse {
    let counts = result.counts;
    TrashCleanupResponse {
        operation_id: result.operation_id,
        reviewed: result.reviewed,
        counts: TrashCleanupCountsWire {
            cleaned: counts.cleaned,
            changed: counts.changed,
            present: counts.present,
            blocked: counts.blocked,
            unavailable: counts.unavailable,
            inspection_failed: counts.inspection_failed,
        },
        items: result
            .items
            .into_iter()
            .map(|item| TrashCleanupItemWire {
                photo_id: item.photo_id,
                original_location: item.relative_path.to_string(),
                original_kind: original_kind(&item.kind),
                state: match item.state {
                    TrashCleanupItemState::Cleaned => "cleaned",
                    TrashCleanupItemState::Changed => "changed",
                    TrashCleanupItemState::Present => "present",
                    TrashCleanupItemState::Blocked => "blocked",
                    TrashCleanupItemState::Unavailable => "unavailable",
                    TrashCleanupItemState::InspectionFailed => "inspection-failed",
                },
                message: item.message,
                blockers: item
                    .blockers
                    .into_iter()
                    .map(|blocker| TrashCleanupBlockerWire {
                        kind: blocker.kind,
                        name: blocker.name,
                        action: blocker.action,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// One captured Trash listing slice and its continuation evidence.
pub(crate) struct TrashPageCapture {
    pub captured: Vec<TrashInspectionRecord>,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub evaluated_at: SystemTime,
    pub expires_at: Option<SystemTime>,
    pub next_cursor: Option<String>,
    pub operation: Option<(String, usize)>,
}

impl Application {
    pub(crate) async fn trash_page(
        &self,
        page: TrashPageCapture,
    ) -> Result<TrashListResponse, ServerError> {
        let TrashPageCapture {
            captured,
            start,
            limit,
            total,
            evaluated_at,
            expires_at,
            next_cursor,
            operation,
        } = page;
        let operation = operation.map(|(operation_id, removed)| PhotoOperationRemainderWire {
            operation_id,
            removed,
        });
        let _publication = self.shared.publication.lock().await;
        let current = self
            .library
            .trash_inspection()
            .await?
            .into_iter()
            .map(|item| (item.photo_id.clone(), item))
            .collect::<std::collections::HashMap<_, _>>();
        let facts = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let published = guard.as_ref().ok_or(ServerError::NotPublished)?;
            captured
                .into_iter()
                .map(|record| {
                    let photo = published
                        .photos_by_id
                        .get(&record.photo_id)
                        .and_then(|position| published.snapshot.photos.get(*position));
                    let state = if photo.is_none() {
                        "unavailable"
                    } else if current.get(&record.photo_id).is_some_and(|now| {
                        now.removed_at_ms == record.removed_at_ms
                            && now.original_id == record.original_id
                            && now.relative_path == record.relative_path
                    }) {
                        "captured"
                    } else {
                        "changed"
                    };
                    let facts = photo.map(|photo| {
                        let originals = published
                            .originals_by_id
                            .get(&photo.original_id)
                            .and_then(|position| published.snapshot.originals.get(*position))
                            .cloned()
                            .into_iter()
                            .collect();
                        PreviewFacts::from_records(photo.clone(), originals)
                    });
                    let available = current
                        .get(&record.photo_id)
                        .filter(|now| {
                            now.original_id == record.original_id
                                && now.relative_path == record.relative_path
                        })
                        .map_or(record.available, |now| now.available);
                    (record, state, facts, available)
                })
                .collect::<Vec<_>>()
        };
        let mut photos = Vec::with_capacity(facts.len());
        for (record, state, facts, available) in facts {
            let photo = if let Some(facts) = facts {
                let (preview_url, thumbnail_url) = self.derivative_urls(&facts).await;
                let originals_by_id = facts
                    .originals
                    .iter()
                    .enumerate()
                    .map(|(position, original)| (original.id.clone(), position))
                    .collect();
                Some(photo_summary_indexed_with_url(
                    &facts.photo,
                    &facts.originals,
                    &originals_by_id,
                    preview_url,
                    thumbnail_url,
                ))
            } else {
                None
            };
            photos.push(TrashItemWire {
                photo_id: record.photo_id,
                removed_at_ms: record.removed_at_ms,
                original_id: record.original_id,
                original_location: record.relative_path.to_string(),
                original_kind: original_kind(&record.kind),
                original_size: available.then_some(record.size),
                original_available: available,
                state,
                photo,
            });
        }
        Ok(TrashListResponse {
            start,
            limit,
            total,
            cleanup_maximum: slipstream_core::TRASH_CLEANUP_MAX,
            restore_maximum: slipstream_core::PHOTO_REMOVAL_MAX,
            evaluated_at: crate::queries::format_time(evaluated_at),
            expires_at: expires_at.map(crate::queries::format_time),
            next_cursor,
            library_root: self.library_root.to_string_lossy().into_owned(),
            namespace: "service",
            operation: operation.map(|operation| PhotoOperationRemainderWire {
                operation_id: operation.operation_id,
                removed: operation.removed,
            }),
            photos,
        })
    }

    pub(crate) async fn preview_trash_cleanup(
        self: &Arc<Self>,
        mut targets: Vec<TrashCleanupTarget>,
    ) -> Result<TrashCleanupPreviewResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let workspaces = Arc::clone(&self.workspaces);
        let (guard, retained) = tokio::task::spawn_blocking(move || {
            let map_error = |error: crate::workspace::WorkspaceError| {
                if error.code == "workspace_busy" {
                    ServerError::WorkspaceBusy
                } else {
                    ServerError::Export(error.to_string())
                }
            };
            let guard = workspaces.recovery_guard().map_err(map_error)?;
            let retained = workspaces
                .retained_workspace_photo_references_under_recovery_guard()
                .map_err(map_error)?;
            Ok::<_, ServerError>((guard, retained))
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        for target in &mut targets {
            for (_, workspace_id) in retained
                .iter()
                .filter(|(photo_id, _)| photo_id == &target.photo_id)
            {
                target.blockers.push(slipstream_core::TrashCleanupBlocker {
                    kind: "workspace".to_owned(),
                    name: workspace_id.clone(),
                    action: "Remove this retained work before cleaning up the record.".to_owned(),
                });
            }
        }
        let result = self.library.preview_trash_cleanup(targets).await?;
        drop(guard);
        Ok(preview_response(result))
    }

    pub(crate) async fn cleanup_trash(
        self: &Arc<Self>,
        operation_id: String,
        targets: Vec<TrashCleanupTarget>,
    ) -> Result<TrashCleanupResponse, ServerError> {
        let application = Arc::clone(self);
        tokio::spawn(async move { application.cleanup_trash_inner(operation_id, targets).await })
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
    }

    async fn cleanup_trash_inner(
        &self,
        operation_id: String,
        mut targets: Vec<TrashCleanupTarget>,
    ) -> Result<TrashCleanupResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        // A known attempt replays before inspecting workspace state.
        if self
            .library
            .read_trash_cleanup(operation_id.clone())
            .await
            .is_ok()
        {
            let result = self.library.cleanup_trash(operation_id, targets).await?;
            let cleaned = result
                .items
                .iter()
                .filter(|item| item.state == TrashCleanupItemState::Cleaned)
                .map(|item| item.photo_id.clone())
                .collect::<Vec<_>>();
            self.shared.patch_trash_cleanup(&cleaned);
            return Ok(cleanup_response(result));
        }
        let workspaces = Arc::clone(&self.workspaces);
        let (guard, retained) = tokio::task::spawn_blocking(move || {
            let map_error = |error: crate::workspace::WorkspaceError| {
                if error.code == "workspace_busy" {
                    ServerError::WorkspaceBusy
                } else {
                    ServerError::Export(error.to_string())
                }
            };
            let guard = workspaces.recovery_guard().map_err(map_error)?;
            let retained = workspaces
                .retained_workspace_photo_references_under_recovery_guard()
                .map_err(map_error)?;
            Ok::<_, ServerError>((guard, retained))
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        for target in &mut targets {
            for (_, workspace_id) in retained
                .iter()
                .filter(|(photo_id, _)| photo_id == &target.photo_id)
            {
                target.blockers.push(slipstream_core::TrashCleanupBlocker {
                    kind: "workspace".to_owned(),
                    name: workspace_id.clone(),
                    action: "Remove this retained work before cleaning up the record.".to_owned(),
                });
            }
        }
        let result = self.library.cleanup_trash(operation_id, targets).await?;
        drop(guard);
        let cleaned = result
            .items
            .iter()
            .filter(|item| item.state == TrashCleanupItemState::Cleaned)
            .map(|item| item.photo_id.clone())
            .collect::<Vec<_>>();
        self.shared.patch_trash_cleanup(&cleaned);
        Ok(cleanup_response(result))
    }

    pub(crate) async fn read_trash_cleanup(
        &self,
        operation_id: String,
    ) -> Result<TrashCleanupResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        Ok(cleanup_response(
            self.library.read_trash_cleanup(operation_id).await?,
        ))
    }
}
