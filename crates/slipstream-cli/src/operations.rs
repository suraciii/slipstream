use super::*;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Status,
    ProcessingModules,
    ProcessingArtifact,
    ProcessingArtifactDownload,
    PhotosProcessingExportStatus,
    PhotosProcessingExportCancel,
    PhotosProcessingRecipeGet,
    PhotosProcessingRecipeSave,
    PhotosProcessingRecipeAuto,
    PhotosProcessingPreview,
    PhotosProcessingExport,
    PhotosProcessingRecipeRebind,
    PhotosProcessingExportList,
    PhotosProcessingExportRetry,
    PhotosHistoricalExportDownload,
    PhotosEditGet,
    PhotosEditSet,
    PhotosEditReset,
    PhotosEditPreview,
    PhotosEditExport,
    PhotosEditExportStatus,
    PhotosProxyGet,
    PhotosProxyCreate,
    PhotosProxyRemove,
    LibraryCheck,
    FoldersList,
    AlbumsList,
    AlbumsGet,
    PhotosList,
    PhotosGet,
    PhotosPreview,
    PhotosSet,
    PhotosRemove,
    PhotosRemovalInspect,
    PhotosRestore,
    PhotosRestoreInspect,
    PhotosMetadata,
    PhotosMetadataSave,
    AlbumsCreate,
    AlbumsRename,
    AlbumsDelete,
    AlbumsAdd,
    AlbumsRemove,
    AlbumsReorder,
    TrashList,
    TrashReview,
    TrashDelete,
    TrashRead,
    RecoveryUnavailable,
    RecoveryPropose,
    RecoveryApply,
}

impl Operation {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::ProcessingModules => "processing-modules",
            Self::ProcessingArtifact => "processing-artifact",
            Self::ProcessingArtifactDownload => "processing-artifact-download",
            Self::PhotosProcessingPreview => "photos-processing-preview",
            Self::PhotosProcessingRecipeGet => "photos-processing-recipe-get",
            Self::PhotosProcessingRecipeSave => "photos-processing-recipe-save",
            Self::PhotosProcessingRecipeAuto => "photos.processing-recipe.auto",
            Self::PhotosProcessingExport => "photos-processing-export",
            Self::PhotosProcessingExportStatus => "photos-processing-export-status",
            Self::PhotosProcessingExportCancel => "photos-processing-export-cancel",
            Self::PhotosProcessingRecipeRebind => "photos-processing-recipe-rebind",
            Self::PhotosProcessingExportList => "photos-processing-export-list",
            Self::PhotosProcessingExportRetry => "photos-processing-export-retry",
            Self::PhotosHistoricalExportDownload => "photos-historical-export-download",
            Self::PhotosEditGet => "photos-edit-get",
            Self::PhotosEditSet => "photos-edit-set",
            Self::PhotosEditReset => "photos-edit-reset",
            Self::PhotosEditPreview => "photos-edit-preview",
            Self::PhotosEditExport => "photos-edit-export",
            Self::PhotosEditExportStatus => "photos-edit-export-status",
            Self::LibraryCheck => "library-check",
            Self::PhotosProxyGet => "photos-proxy-get",
            Self::PhotosProxyCreate => "photos-proxy-create",
            Self::PhotosProxyRemove => "photos-proxy-remove",
            Self::FoldersList => "folders-list",
            Self::AlbumsList => "albums-list",
            Self::AlbumsGet => "albums-get",
            Self::PhotosList => "photos-list",
            Self::PhotosGet => "photos-get",
            Self::PhotosPreview => "photos-preview",
            Self::PhotosSet => "photos-set",
            Self::PhotosRemove => "photos-remove",
            Self::PhotosRemovalInspect => "photos-removal-operation",
            Self::PhotosRestore => "photos-restore",
            Self::PhotosRestoreInspect => "photos-restore-operation",
            Self::PhotosMetadata => "photos-metadata",
            Self::PhotosMetadataSave => "photos-metadata-save",
            Self::AlbumsCreate => "albums-create",
            Self::AlbumsRename => "albums-rename",
            Self::AlbumsDelete => "albums-delete",
            Self::AlbumsAdd => "albums-add",
            Self::AlbumsRemove => "albums-remove",
            Self::AlbumsReorder => "albums-reorder",
            Self::TrashList => "trash-list",
            Self::TrashReview => "trash-review",
            Self::TrashDelete => "trash-delete",
            Self::TrashRead => "trash-operation",
            Self::RecoveryUnavailable => "recovery-unavailable",
            Self::RecoveryPropose => "recovery-propose",
            Self::RecoveryApply => "recovery-apply",
        }
    }
}

pub(crate) fn command_operation(command: &Command) -> Operation {
    match command {
        Command::Status => Operation::Status,
        Command::Processing { command } => match command {
            ProcessingCommand::Modules => Operation::ProcessingModules,
            ProcessingCommand::Artifact { .. } => Operation::ProcessingArtifact,
            ProcessingCommand::ArtifactDownload { .. } => Operation::ProcessingArtifactDownload,
        },
        Command::Library { .. } => Operation::LibraryCheck,
        Command::Folders { .. } => Operation::FoldersList,
        Command::Albums { command } => match command {
            AlbumCommand::List(_) => Operation::AlbumsList,
            AlbumCommand::Get { .. } => Operation::AlbumsGet,
            AlbumCommand::Create { .. } => Operation::AlbumsCreate,
            AlbumCommand::Rename { .. } => Operation::AlbumsRename,
            AlbumCommand::Delete { .. } => Operation::AlbumsDelete,
            AlbumCommand::Add(_) => Operation::AlbumsAdd,
            AlbumCommand::Remove(_) => Operation::AlbumsRemove,
            AlbumCommand::Reorder(_) => Operation::AlbumsReorder,
        },
        Command::Photos { command } => match command {
            PhotoCommand::List(_) => Operation::PhotosList,
            PhotoCommand::Get { .. } => Operation::PhotosGet,
            PhotoCommand::Preview { .. } => Operation::PhotosPreview,
            PhotoCommand::Proxy { command } => match command {
                development_proxy::DevelopmentProxyCommand::Get { .. } => Operation::PhotosProxyGet,
                development_proxy::DevelopmentProxyCommand::Create { .. } => {
                    Operation::PhotosProxyCreate
                }
                development_proxy::DevelopmentProxyCommand::Remove { .. } => {
                    Operation::PhotosProxyRemove
                }
            },
            PhotoCommand::Edit { command } => match command {
                edit::EditCommand::Get { .. } => Operation::PhotosEditGet,
                edit::EditCommand::Set { .. } => Operation::PhotosEditSet,
                edit::EditCommand::Reset { .. } => Operation::PhotosEditReset,
                edit::EditCommand::Preview { .. } => Operation::PhotosEditPreview,
                edit::EditCommand::Export { .. } => Operation::PhotosEditExport,
                edit::EditCommand::ExportStatus { .. } => Operation::PhotosEditExportStatus,
            },
            PhotoCommand::ProcessingRecipe { command } => match command {
                development::ProcessingRecipeCommand::Get { .. } => {
                    Operation::PhotosProcessingRecipeGet
                }
                development::ProcessingRecipeCommand::Save(_) => {
                    Operation::PhotosProcessingRecipeSave
                }
                development::ProcessingRecipeCommand::Auto(_) => {
                    Operation::PhotosProcessingRecipeAuto
                }
                development::ProcessingRecipeCommand::Rebind(_) => {
                    Operation::PhotosProcessingRecipeRebind
                }
            },
            PhotoCommand::ProcessingPreview { .. } => Operation::PhotosProcessingPreview,
            PhotoCommand::ProcessingExport(_) => Operation::PhotosProcessingExport,
            PhotoCommand::ProcessingExportStatus { .. } => Operation::PhotosProcessingExportStatus,
            PhotoCommand::ProcessingExportCancel { .. } => Operation::PhotosProcessingExportCancel,
            PhotoCommand::ProcessingExportList { .. } => Operation::PhotosProcessingExportList,
            PhotoCommand::ProcessingExportRetry { .. } => Operation::PhotosProcessingExportRetry,
            PhotoCommand::HistoricalExportDownload { .. } => {
                Operation::PhotosHistoricalExportDownload
            }
            PhotoCommand::Set(_) => Operation::PhotosSet,
            PhotoCommand::Remove(_) => Operation::PhotosRemove,
            PhotoCommand::RemovalOperation { .. } => Operation::PhotosRemovalInspect,
            PhotoCommand::Restore(_) => Operation::PhotosRestore,
            PhotoCommand::RestoreOperation { .. } => Operation::PhotosRestoreInspect,
            PhotoCommand::Metadata { .. } => Operation::PhotosMetadata,
            PhotoCommand::MetadataSave(_) => Operation::PhotosMetadataSave,
        },
        Command::Trash { command } => match command {
            TrashCommand::List(_) => Operation::TrashList,
            TrashCommand::Review(_) => Operation::TrashReview,
            TrashCommand::Delete { .. } => Operation::TrashDelete,
            TrashCommand::Operation { .. } => Operation::TrashRead,
        },
        Command::Recovery { command } => match command {
            RecoveryCommand::Unavailable(_) => Operation::RecoveryUnavailable,
            RecoveryCommand::Propose(_) => Operation::RecoveryPropose,
            RecoveryCommand::Apply(_) => Operation::RecoveryApply,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MembershipKind {
    Add,
    Remove,
    Reorder,
}

impl MembershipKind {
    pub(crate) fn operation(self) -> Operation {
        match self {
            Self::Add => Operation::AlbumsAdd,
            Self::Remove => Operation::AlbumsRemove,
            Self::Reorder => Operation::AlbumsReorder,
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Reorder => "reorder",
        }
    }

    pub(crate) fn limit_name(self) -> &'static str {
        match self {
            Self::Reorder => "albumReorderMembersMaximum",
            Self::Add | Self::Remove => "mutationPhotoIdsMaximum",
        }
    }
}

/// Checks that two returned ID arrays are an order-preserving partition of the
/// submitted IDs: no omission, no duplication, and request order preserved.
pub(crate) fn request_order_partition(
    submitted: &[String],
    first: &[String],
    second: &[String],
) -> bool {
    let mut assignment = std::collections::HashMap::<&str, usize>::new();
    for (index, ids) in [first, second].into_iter().enumerate() {
        for id in ids {
            if assignment.insert(id.as_str(), index).is_some() {
                return false;
            }
        }
    }
    if assignment.len() != submitted.len() {
        return false;
    }
    let mut consumed = [0_usize, 0_usize];
    for id in submitted {
        let Some(&assigned) = assignment.get(id.as_str()) else {
            return false;
        };
        if [first, second][assigned]
            .get(consumed[assigned])
            .is_none_or(|expected| expected != id)
        {
            return false;
        }
        consumed[assigned] += 1;
    }
    consumed == [first.len(), second.len()]
}

pub(crate) fn confirmed<T>(
    result: Result<T, ()>,
    identity: &MutationIdentity,
) -> Result<T, CommandFailure> {
    result.map_err(|()| CommandFailure::unknown(identity))
}

pub(crate) async fn membership_mutation(
    kind: MembershipKind,
    args: &AlbumMembershipArgs,
    photo_ids: Vec<String>,
    client: &ServiceClient,
    admission: &AdmissionState,
) -> Result<Value, CommandFailure> {
    let identity = MutationIdentity {
        operation: kind.operation(),
        photo_ids: photo_ids.clone(),
        album_id: Some(args.album_id.clone()),
        album_name: None,
        mappings: Vec::new(),
    };
    let result: Value = client
        .mutation(
            &identity,
            admission,
            client.endpoint(&["api", "albums", &args.album_id, "changes"]),
            json!({
                "operation": kind.wire(),
                "photoIds": photo_ids,
                "ifVersion": args.if_version,
            }),
        )
        .await?;
    confirmed_membership_result(kind, &identity, result, &client.origin)
}

/// Validates one confirmed membership result against the submitted request
/// and renders the CLI reference data shape.
pub(crate) fn confirmed_membership_result(
    kind: MembershipKind,
    identity: &MutationIdentity,
    result: Value,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let unknown = || CommandFailure::unknown(identity);
    match kind {
        MembershipKind::Add => {
            let result: AlbumAddWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if !request_order_partition(
                &identity.photo_ids,
                &result.added_photo_ids,
                &result.already_member_photo_ids,
            ) {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "addedPhotoIds": result.added_photo_ids,
                "alreadyMemberPhotoIds": result.already_member_photo_ids,
            }))
        }
        MembershipKind::Remove => {
            let result: AlbumRemoveWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if result.saved_photo_id.as_deref().is_some_and(str::is_empty)
                || !request_order_partition(
                    &identity.photo_ids,
                    &result.removed_photo_ids,
                    &result.already_absent_photo_ids,
                )
            {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "removedPhotoIds": result.removed_photo_ids,
                "alreadyAbsentPhotoIds": result.already_absent_photo_ids,
                "savedPhotoId": result.saved_photo_id,
            }))
        }
        MembershipKind::Reorder => {
            let result: AlbumReorderWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if result.ordered_photo_ids != identity.photo_ids {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "orderedPhotoIds": result.ordered_photo_ids,
                "reordered": result.reordered,
            }))
        }
    }
}

/// Validates one confirmed decision batch against the submitted request and
/// derives the CLI reference data shape, partition, and exit code. Every
/// result must match its requested Photo and outcome-specific key set, the
/// reported counts must count those outcomes, and every changed or
/// unchanged result must repeat the requested decision value in its
/// current snapshot; anything else is an unknown outcome rather than a
/// claimed partition.
pub(crate) fn confirmed_decision_result(
    identity: &MutationIdentity,
    prepared: &PreparedDecision,
    result: PhotoDecisionWire,
) -> Result<Value, CommandFailure> {
    let unknown = || CommandFailure::unknown(identity);
    if result.results.len() != prepared.photos.len() {
        return Err(unknown());
    }
    let mut counted = [0_usize; 4];
    let mut first_conflict: Option<(String, String)> = None;
    let mut first_missing: Option<String> = None;
    let mut items = Vec::with_capacity(result.results.len());
    // A changed or unchanged result reports the requested decision value,
    // so any other current value is an untrustworthy response.
    let echoed_request = |current: &PhotoDecisionSnapshotWire| match prepared.field {
        DecisionField::SelectionState => serde_json::to_value(&current.selection_state)
            .is_ok_and(|snapshot| snapshot == prepared.value),
        DecisionField::Rating => prepared.value.as_u64() == Some(u64::from(current.rating)),
    };
    for (item, submitted_photo) in result.results.into_iter().zip(&prepared.photos) {
        if item.photo_id != submitted_photo.photo_id {
            return Err(unknown());
        }
        let mut value = json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
        });
        let current_valid = |current: &PhotoDecisionSnapshotWire| {
            !current.decision_version.is_empty() && current.rating <= 5
        };
        match item.outcome.as_str() {
            "changed" => {
                let (Some(prior), Some(current)) = (&item.prior, &item.current) else {
                    return Err(unknown());
                };
                if prior.rating > 5 || !current_valid(current) || !echoed_request(current) {
                    return Err(unknown());
                }
                counted[0] += 1;
                value["prior"] = json!({
                    "selectionState": prior.selection_state,
                    "rating": prior.rating,
                });
                value["current"] = json!({
                    "selectionState": current.selection_state,
                    "rating": current.rating,
                    "decisionVersion": current.decision_version,
                });
            }
            "unchanged" | "conflict" => {
                if item.prior.is_some() {
                    return Err(unknown());
                }
                let Some(current) = &item.current else {
                    return Err(unknown());
                };
                if !current_valid(current) {
                    return Err(unknown());
                }
                if item.outcome == "unchanged" {
                    if !echoed_request(current) {
                        return Err(unknown());
                    }
                    counted[1] += 1;
                } else {
                    counted[2] += 1;
                    if first_conflict.is_none() {
                        first_conflict =
                            Some((item.photo_id.clone(), current.decision_version.clone()));
                    }
                }
                value["current"] = json!({
                    "selectionState": current.selection_state,
                    "rating": current.rating,
                    "decisionVersion": current.decision_version,
                });
            }
            "missing" => {
                if item.prior.is_some() || item.current.is_some() {
                    return Err(unknown());
                }
                counted[3] += 1;
                if first_missing.is_none() {
                    first_missing = Some(item.photo_id.clone());
                }
            }
            _ => return Err(unknown()),
        }
        items.push(value);
    }
    let [changed, unchanged, conflict, missing] = counted;
    if (changed, unchanged, conflict, missing)
        != (
            result.counts.changed,
            result.counts.unchanged,
            result.counts.conflict,
            result.counts.missing,
        )
    {
        return Err(unknown());
    }
    let counts = json!({
        "changed": changed,
        "unchanged": unchanged,
        "conflict": conflict,
        "missing": missing,
    });
    let data = json!({ "results": items, "counts": counts });
    if conflict + missing == 0 {
        return Ok(data);
    }
    if changed + unchanged > 0 {
        return Err(CommandFailure::photo_batch_partial(&counts).with_data(data));
    }
    if conflict > 0 {
        let (reference, current_version) =
            first_conflict.expect("a conflicting result was counted");
        return Err(
            CommandFailure::photo_batch_conflict(&reference, &current_version).with_data(data),
        );
    }
    let reference = first_missing.expect("a missing result was counted");
    Err(CommandFailure::photo_batch_missing(&reference).with_data(data))
}

pub(crate) fn confirmed_removal_result(
    identity: &MutationIdentity,
    operation_id: &str,
    prepared: &PreparedRemoval,
    result: PhotoRemovalWire,
) -> Result<Value, CommandFailure> {
    if result.operation_id != operation_id || result.results.len() != prepared.photos.len() {
        return Err(CommandFailure::unknown(identity));
    }
    let submitted = prepared
        .photos
        .iter()
        .map(|photo| photo.photo_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut counts = PhotoRemovalCountsWire {
        removed: 0,
        changed_elsewhere: 0,
        missing: 0,
        already_removed: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(result.results.len());
    for item in result.results {
        if !submitted.contains(item.photo_id.as_str()) || !seen.insert(item.photo_id.clone()) {
            return Err(CommandFailure::unknown(identity));
        }
        match item.outcome.as_str() {
            "removed" => {
                if item.removed_at_ms.is_none_or(|value| value < 0) {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.removed += 1;
            }
            "changed-elsewhere" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.changed_elsewhere += 1;
            }
            "unavailable" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.missing += 1;
            }
            "already-removed" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.already_removed += 1;
            }
            _ => return Err(CommandFailure::unknown(identity)),
        };
        items.push(json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
            "removedAtMs": item.removed_at_ms,
        }));
    }
    if seen.len() != submitted.len()
        || counts.removed != result.counts.removed
        || counts.changed_elsewhere != result.counts.changed_elsewhere
        || counts.missing != result.counts.missing
        || counts.already_removed != result.counts.already_removed
    {
        return Err(CommandFailure::unknown(identity));
    }
    let data = json!({
        "operationId": result.operation_id,
        "counts": {
            "removed": counts.removed,
            "changedElsewhere": counts.changed_elsewhere,
            "missing": counts.missing,
            "alreadyRemoved": counts.already_removed,
        },
        "results": items,
    });
    if counts.changed_elsewhere == 0 && counts.missing == 0 {
        return Ok(data);
    }
    let code = if counts.changed_elsewhere > 0 {
        "conflict"
    } else {
        "not_found"
    };
    let message = if code == "conflict" {
        "Read current Photo evidence before submitting a replacement removal."
    } else {
        "Query Photos and use current Photo IDs before submitting a replacement removal."
    };
    let effect = if counts.removed + counts.already_removed > 0 {
        "partial"
    } else {
        "none"
    };
    Err(CommandFailure::from_payload(
        if code == "conflict" { 4 } else { 3 },
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: effect.to_owned(),
            details: json!({"operationId": operation_id}),
        },
    )
    .with_data(data))
}

pub(crate) fn confirmed_restore_result(
    identity: &MutationIdentity,
    operation_id: &str,
    prepared: &PreparedRestore,
    result: PhotoRestoreWire,
) -> Result<Value, CommandFailure> {
    if result.operation_id != operation_id || result.results.len() != prepared.markers.len() {
        return Err(CommandFailure::unknown(identity));
    }
    let submitted = prepared
        .markers
        .iter()
        .map(|marker| marker.photo_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut counts = PhotoRestoreCountsWire {
        restored: 0,
        already_active: 0,
        changed_elsewhere: 0,
        missing: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(result.results.len());
    for item in result.results {
        if !submitted.contains(item.photo_id.as_str()) || !seen.insert(item.photo_id.clone()) {
            return Err(CommandFailure::unknown(identity));
        }
        match item.outcome.as_str() {
            "restored" => counts.restored += 1,
            "already-active" => counts.already_active += 1,
            "changed-elsewhere" => counts.changed_elsewhere += 1,
            "unavailable" => counts.missing += 1,
            _ => return Err(CommandFailure::unknown(identity)),
        }
        items.push(json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
        }));
    }
    if seen.len() != submitted.len()
        || counts.restored != result.counts.restored
        || counts.already_active != result.counts.already_active
        || counts.changed_elsewhere != result.counts.changed_elsewhere
        || counts.missing != result.counts.missing
    {
        return Err(CommandFailure::unknown(identity));
    }
    let data = json!({
        "operationId": result.operation_id,
        "counts": {
            "restored": counts.restored,
            "alreadyActive": counts.already_active,
            "changedElsewhere": counts.changed_elsewhere,
            "missing": counts.missing,
        },
        "results": items,
    });
    if counts.changed_elsewhere == 0 && counts.missing == 0 {
        return Ok(data);
    }
    let code = if counts.changed_elsewhere > 0 {
        "conflict"
    } else {
        "not_found"
    };
    let message = if code == "conflict" {
        "Read current Trash evidence before submitting a replacement Restore."
    } else {
        "Query Trash and use current Photo IDs before submitting a replacement Restore."
    };
    let effect = if counts.restored + counts.already_active > 0 {
        "partial"
    } else {
        "none"
    };
    Err(CommandFailure::from_payload(
        if code == "conflict" { 4 } else { 3 },
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: effect.to_owned(),
            details: json!({"operationId": operation_id}),
        },
    )
    .with_data(data))
}

pub(crate) fn restore_wire_value(
    result: PhotoRestoreWire,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let total = result
        .counts
        .restored
        .checked_add(result.counts.already_active)
        .and_then(|value| value.checked_add(result.counts.changed_elsewhere))
        .and_then(|value| value.checked_add(result.counts.missing));
    if result.operation_id.is_empty() || total != Some(result.results.len()) {
        return Err(CommandFailure::transport(operation));
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &result.results {
        if item.photo_id.is_empty() || !ids.insert(item.photo_id.as_str()) {
            return Err(CommandFailure::transport(operation));
        }
        if !matches!(
            item.outcome.as_str(),
            "restored" | "already-active" | "changed-elsewhere" | "unavailable"
        ) {
            return Err(CommandFailure::transport(operation));
        }
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::transport(operation))
}
pub(crate) fn removal_wire_value(
    result: PhotoRemovalWire,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    if result.operation_id.is_empty()
        || result.counts.removed
            + result.counts.changed_elsewhere
            + result.counts.missing
            + result.counts.already_removed
            != result.results.len()
    {
        return Err(CommandFailure::transport(operation));
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &result.results {
        if item.photo_id.is_empty() || !ids.insert(item.photo_id.as_str()) {
            return Err(CommandFailure::transport(operation));
        }
        match item.outcome.as_str() {
            "removed" if item.removed_at_ms.is_some_and(|value| value >= 0) => {}
            "changed-elsewhere" | "unavailable" | "already-removed"
                if item.removed_at_ms.is_none() => {}
            _ => return Err(CommandFailure::transport(operation)),
        }
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::transport(operation))
}

pub(crate) fn preview_valid(preview: &PreviewFacts) -> bool {
    match preview.state {
        PreviewState::Ready => {
            preview.source.is_some()
                && preview.source_revision.as_deref().is_some_and(|value| {
                    !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES
                })
                && preview.width.is_some_and(|value| value > 0)
                && preview.height.is_some_and(|value| value > 0)
                && preview.detail_limited.is_some()
        }
        _ => {
            preview.source.is_none()
                && preview.source_revision.is_none()
                && preview.width.is_none()
                && preview.height.is_none()
                && preview.detail_limited.is_none()
        }
    }
}

pub(crate) fn photo_value(photo: PhotoItem, origin: &Url) -> Result<Value, ()> {
    validate_nonempty(&photo.id)?;
    validate_nonempty(&photo.decision_version)?;
    if photo.rating > 5
        || photo
            .capture_time
            .as_deref()
            .is_some_and(|value| !valid_camera_time(value))
        || !preview_valid(&photo.preview)
    {
        return Err(());
    }
    let mut value = serde_json::to_value(&photo).map_err(|_| ())?;
    let object = value.as_object_mut().ok_or(())?;
    object.remove("webPath");
    object.insert(
        "webUrl".to_owned(),
        Value::String(web_url(origin, &photo.web_path)?),
    );
    Ok(value)
}

/// One validated `RecoveryItem` as printed data: the reviewed identity with
/// an absolute Web URL. A field the service did not substantiate fails the
/// whole read instead of printing a partial item.
pub(crate) fn recovery_item_value(item: RecoveryItemWire, origin: &Url) -> Result<Value, ()> {
    validate_nonempty(&item.original_id)?;
    validate_nonempty(&item.photo_id)?;
    if item.location.is_empty() || item.rating > 5 {
        return Err(());
    }
    Ok(json!({
        "state": item.state,
        "originalId": item.original_id,
        "photoId": item.photo_id,
        "location": item.location,
        "kind": item.kind,
        "rating": item.rating,
        "selectionState": item.selection_state,
        "fingerprintEnrolled": item.fingerprint_enrolled,
        "albumCount": item.album_count,
        "webUrl": web_url(origin, &item.web_url)?,
    }))
}

/// One validated `RecoveryMapping` as printed data, with the same
/// absolute-URL and closed-value rules as the unavailable review items.
pub(crate) fn recovery_mapping_value(mapping: RecoveryMappingWire) -> Result<Value, ()> {
    validate_nonempty(&mapping.mapping_id)?;
    validate_nonempty(&mapping.original_id)?;
    validate_nonempty(&mapping.photo_id)?;
    if mapping.from_location.is_empty()
        || mapping.to_location.is_empty()
        || !valid_original_location(&mapping.to_location)
    {
        return Err(());
    }
    if let Some(retire) = &mapping.retire {
        validate_nonempty(&retire.photo_id)?;
        validate_nonempty(&retire.original_id)?;
        if retire.location.is_empty() {
            return Err(());
        }
    }
    Ok(json!({
        "mappingId": mapping.mapping_id,
        "originalId": mapping.original_id,
        "photoId": mapping.photo_id,
        "fromLocation": mapping.from_location,
        "toLocation": mapping.to_location,
        "kind": mapping.kind,
        "outcome": mapping.outcome,
        "verified": mapping.verified,
        "blockedReason": mapping.blocked_reason,
        "retire": mapping.retire,
    }))
}

/// Validates one confirmed apply result against the submitted batch: the
/// whole batch committed, in request order, with substantiated fields. A
/// result that cannot prove the commit is an unknown outcome, never success.
pub(crate) fn recovery_apply_value(
    identity: &MutationIdentity,
    result: RecoveryApplyData,
    prepared: &PreparedRecoveryApply,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let submitted = prepared.identities.len() as u64;
    let mut mappings = Vec::with_capacity(result.mappings.len());
    for (applied, submitted_mapping) in result.mappings.iter().zip(&prepared.identities) {
        validate_nonempty(&applied.original_id)
            .and_then(|()| validate_nonempty(&applied.photo_id))
            .map_err(|()| CommandFailure::unknown(identity))?;
        if applied.from_location.is_empty() || !valid_original_location(&applied.to_location) {
            return Err(CommandFailure::unknown(identity));
        }
        if let Some(retired) = &applied.retired {
            validate_nonempty(&retired.photo_id)
                .and_then(|()| validate_nonempty(&retired.original_id))
                .map_err(|()| CommandFailure::unknown(identity))?;
            if retired.location.is_empty() {
                return Err(CommandFailure::unknown(identity));
            }
        }
        if applied.original_id != submitted_mapping["originalId"].as_str().unwrap_or("")
            || applied.to_location != submitted_mapping["newLocation"].as_str().unwrap_or("")
        {
            return Err(CommandFailure::unknown(identity));
        }
        mappings.push(json!({
            "originalId": applied.original_id,
            "photoId": applied.photo_id,
            "fromLocation": applied.from_location,
            "toLocation": applied.to_location,
            "webUrl": web_url(origin, &applied.web_url).map_err(|()| CommandFailure::unknown(identity))?,
            "retired": applied.retired,
        }));
    }
    if result.applied_mappings != submitted
        || result.refused_mappings != 0
        || result.mappings.len() as u64 != submitted
    {
        return Err(CommandFailure::unknown(identity));
    }
    Ok(json!({
        "appliedMappings": result.applied_mappings,
        "refusedMappings": result.refused_mappings,
        "unavailablePhotos": result.unavailable_photos,
        "mappings": mappings,
    }))
}

pub(crate) fn list_expiry_valid<T>(list: &ListData<T>, page_limit: usize) -> bool {
    list.items.len() <= page_limit
        && list.total >= list.items.len() as u64
        && valid_utc_time(&list.evaluated_at)
        && list.next_cursor.is_some() == list.expires_at.is_some()
        && list.expires_at.as_deref().is_none_or(valid_utc_time)
}
