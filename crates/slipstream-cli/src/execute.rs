use super::*;
pub(crate) async fn execute(
    cli: &Cli,
    environment: Option<&str>,
    admission: &AdmissionState,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let operation = command_operation(&cli.command);
    validate_command(&cli.command)?;
    let preview_destination = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Preview { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Preview,
            file,
        )?),
        Command::Photos {
            command: PhotoCommand::ProcessingPreview { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::ProcessingPreview,
            file,
        )?),
        Command::Processing {
            command: ProcessingCommand::ArtifactDownload { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Artifact,
            file,
        )?),
        Command::Photos {
            command: PhotoCommand::HistoricalExportDownload { file, .. },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Artifact,
            file,
        )?),
        _ => None,
    };
    let origin = service_origin(cli, environment)?;
    if origin.scheme() == "http" {
        eprintln!(
            "Warning: the HTTP service origin is unencrypted; requests and credentials may be observed in transit."
        );
    }
    let token_path = access_token_path(cli)?;
    let token = read_access_token(token_path).await?;
    // The complete membership and decision documents validate before any
    // network access, so a local input failure can never depend on service
    // reachability.
    let pending_membership = match &cli.command {
        Command::Albums {
            command: AlbumCommand::Add(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Add.limit_name()).await?),
        Command::Albums {
            command: AlbumCommand::Remove(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Remove.limit_name()).await?),
        Command::Albums {
            command: AlbumCommand::Reorder(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Reorder.limit_name()).await?),
        _ => None,
    };
    let pending_decision = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Set(args),
        } => match &args.input {
            Some(input) => Some(read_decision_input(input).await?),
            None => None,
        },
        _ => None,
    };
    let pending_removal = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Remove(args),
        } => Some(read_removal_input(&args.input).await?),
        _ => None,
    };
    let pending_restore = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Restore(args),
        } => match &args.input {
            Some(input) => Some(read_restore_input(input).await?),
            None => None,
        },
        _ => None,
    };
    // The complete save document validates before any network access, so a
    // local input failure can never depend on service reachability.
    let pending_metadata_save = match &cli.command {
        Command::Photos {
            command: PhotoCommand::MetadataSave(args),
        } => Some(prepare_metadata_save(&args.photo_id, &args.input).await?),
        _ => None,
    };
    let pending_trash_review = match &cli.command {
        Command::Trash {
            command: TrashCommand::Review(args),
        } => {
            let photo_ids = match (&args.all, &args.input) {
                (true, None) => Vec::new(),
                (false, Some(input)) => read_trash_ids(input).await?,
                _ => {
                    return Err(CommandFailure::invalid(
                        "input",
                        "Trash review requires --all or --input, but not both.",
                    ));
                }
            };
            let exclude_photo_ids = match &args.exclude_input {
                Some(input) => read_trash_ids(input).await?,
                None => Vec::new(),
            };
            Some(PendingTrashReview {
                photo_ids,
                exclude_photo_ids,
            })
        }
        _ => None,
    };
    let pending_processing_recipe = match &cli.command {
        Command::Photos {
            command: PhotoCommand::ProcessingRecipe { command },
        } => development::prepare_processing(command).await?,
        _ => None,
    };
    let pending_processing_export = match &cli.command {
        Command::Photos {
            command: PhotoCommand::ProcessingExport(args),
        } => Some(development::prepare_processing_export(&args.input).await?),
        Command::Photos {
            command: PhotoCommand::ProcessingExportRetry { input, .. },
        } => Some(development::prepare_processing_export_retry(input).await?),
        _ => None,
    };
    let pending_recovery_apply = match &cli.command {
        Command::Recovery {
            command: RecoveryCommand::Apply(args),
        } => Some(read_recovery_apply(&args.input).await?),
        _ => None,
    };
    let pending_proxy_create = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Proxy { command },
        } => development_proxy::prepare(command).await?,
        _ => None,
    };
    let client = ServiceClient::new(origin, token)?;
    let limits = client.capabilities(operation).await?;

    let result = async {
        match &cli.command {
            Command::Processing { command } => match command {
                ProcessingCommand::Modules => development::modules(&client).await,
                ProcessingCommand::Artifact { artifact_id } => {
                    development::processing_artifact(&client, artifact_id).await
                }
                ProcessingCommand::ArtifactDownload { artifact_id, .. } => {
                    // The staged destination was preflighted so an existing
                    // file is never replaced; a refusal or an
                    // unidentifiable transfer publishes nothing.
                    processing_artifact_download::download(
                        &client,
                        artifact_id,
                        preview_destination.expect("Processing Artifact destination was checked"),
                        publication,
                    )
                    .await
                }
            },
            Command::Photos {
                command: PhotoCommand::ProcessingRecipe { command },
            } => {
                development::execute_processing(
                    &client,
                    admission,
                    command,
                    pending_processing_recipe,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::ProcessingPreview { photo_id, step, .. },
            } => {
                // The staged destination was preflighted so an existing
                // file is never replaced; a refusal or an unidentifiable
                // response publishes nothing.
                processing_preview_download::download(
                    &client,
                    photo_id,
                    step,
                    preview_destination.expect("Processing Preview destination was checked"),
                    publication,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::ProcessingExport(args),
            } => {
                let body = pending_processing_export.ok_or_else(development::unusable_input)?;
                development::execute_processing_export(&client, admission, args, body).await
            }
            Command::Photos {
                command:
                    PhotoCommand::ProcessingExportStatus {
                        photo_id,
                        request_id,
                    },
            } => development::processing_export_status(&client, photo_id, request_id).await,
            Command::Photos {
                command:
                    PhotoCommand::ProcessingExportCancel {
                        photo_id,
                        request_id,
                    },
            } => {
                development::processing_export_cancel(&client, admission, photo_id, request_id)
                    .await
            }
            Command::Photos {
                command: PhotoCommand::ProcessingExportList { photo_id },
            } => development::processing_export_list(&client, photo_id).await,
            Command::Photos {
                command:
                    PhotoCommand::ProcessingExportRetry {
                        photo_id,
                        request_id,
                        ..
                    },
            } => {
                let body = pending_processing_export.ok_or_else(development::unusable_input)?;
                development::execute_processing_export_retry(
                    &client, admission, photo_id, request_id, body,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::HistoricalExportDownload { export_id, .. },
            } => {
                historical_export_download::download(
                    &client,
                    export_id,
                    preview_destination.expect("Historical Export destination was checked"),
                    publication,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::Proxy { command },
            } => {
                development_proxy::execute(&client, admission, command, pending_proxy_create).await
            }
            Command::Status => {
                let data: StatusData = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "status"]),
                        None,
                    )
                    .await?;
                if data.server_version.is_empty()
                    || data.cli_contract_version != CLI_CONTRACT_VERSION
                    || data.publication.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                serde_json::to_value(data).map_err(|_| CommandFailure::transport(operation))
            }
            Command::Library {
                command: LibraryCommand::Check,
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                let scan: ScanStatus = match client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "scan"]),
                        json!({}),
                    )
                    .await
                {
                    Ok(scan) => scan,
                    Err(failure) if failure.payload.code == "library_unavailable" => {
                        let data = json!({ "scan": failure.payload.details["scan"] });
                        return Err(failure.with_data(data));
                    }
                    Err(failure) => return Err(failure),
                };
                Ok(json!({ "scan": scan }))
            }
            Command::Folders {
                command: FolderCommand::List(args),
            } => {
                let mut url = client.endpoint(&["api", "file-locations"]);
                if let Some(cursor) = &args.cursor {
                    url.query_pairs_mut().append_pair("cursor", cursor);
                } else {
                    if let Some(parent) = &args.parent {
                        url.query_pairs_mut().append_pair("parent", parent);
                    }
                    if let Some(limit) = args.limit {
                        url.query_pairs_mut()
                            .append_pair("limit", &limit.to_string());
                    }
                }
                let data: FolderListData = client.json(operation, Method::GET, url, None).await?;
                let page_limit = if args.cursor.is_some() {
                    MAXIMUM_LIST_PAGE
                } else {
                    usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
                };
                if data.expires_at.is_some()
                    || data.items.len() > page_limit
                    || !valid_utc_time(&data.evaluated_at)
                    || data.total < data.items.len() as u64
                    || data.publication.is_empty()
                    || data.next_cursor.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                serde_json::to_value(data).map_err(|_| CommandFailure::transport(operation))
            }
            Command::Albums {
                command: AlbumCommand::List(args),
            } => {
                let mut url = client.endpoint(&["api", "album-summaries"]);
                if let Some(cursor) = &args.cursor {
                    url.query_pairs_mut().append_pair("cursor", cursor);
                } else {
                    if let Some(name) = &args.name {
                        url.query_pairs_mut().append_pair("name", name);
                    }
                    if let Some(photo) = &args.photo {
                        url.query_pairs_mut().append_pair("photoId", photo);
                    }
                    if let Some(limit) = args.limit {
                        url.query_pairs_mut()
                            .append_pair("limit", &limit.to_string());
                    }
                }
                let data: ListData<AlbumListItem> =
                    client.json(operation, Method::GET, url, None).await?;
                let page_limit = if args.cursor.is_some() {
                    MAXIMUM_LIST_PAGE
                } else {
                    usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
                };
                if !list_expiry_valid(&data, page_limit)
                    || data.next_cursor.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                let items = data
                    .items
                    .into_iter()
                    .map(|item| match item {
                        AlbumListItem::Missing(missing) => missing_value(missing),
                        AlbumListItem::Present(album) => album_value(album, &client.origin),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| CommandFailure::transport(operation))?;
                Ok(json!({
                    "items": items,
                    "total": data.total,
                    "nextCursor": data.next_cursor,
                    "evaluatedAt": data.evaluated_at,
                    "expiresAt": data.expires_at,
                }))
            }
            Command::Albums {
                command: AlbumCommand::Get { album_id },
            } => {
                let data: AlbumSummary = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "albums", album_id]),
                        None,
                    )
                    .await?;
                album_value(data, &client.origin).map_err(|_| CommandFailure::transport(operation))
            }
            Command::Albums {
                command: AlbumCommand::Create { name },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: Some(name.clone()),
                    mappings: Vec::new(),
                };
                let result: AlbumCreationWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums"]),
                        json!({ "name": name }),
                    )
                    .await?;
                let album = confirmed(album_value(result.album, &client.origin), &identity)?;
                Ok(json!({ "album": album }))
            }
            Command::Albums {
                command:
                    AlbumCommand::Rename {
                        album_id,
                        name,
                        if_version,
                    },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: Some(album_id.clone()),
                    album_name: Some(name.clone()),
                    mappings: Vec::new(),
                };
                let result: AlbumRenameWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums", album_id, "changes"]),
                        json!({ "operation": "rename", "name": name, "ifVersion": if_version }),
                    )
                    .await?;
                let album = confirmed(album_value(result.album, &client.origin), &identity)?;
                Ok(json!({ "album": album, "renamed": result.renamed }))
            }
            Command::Albums {
                command:
                    AlbumCommand::Delete {
                        album_id,
                        if_version,
                    },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: Some(album_id.clone()),
                    album_name: None,
                    mappings: Vec::new(),
                };
                let result: AlbumDeleteWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums", album_id, "changes"]),
                        json!({ "operation": "delete", "ifVersion": if_version }),
                    )
                    .await?;
                if !result.deleted || result.original_files_changed || result.album_id.is_empty() {
                    return Err(CommandFailure::unknown(&identity));
                }
                Ok(json!({
                    "albumId": result.album_id,
                    "deleted": true,
                    "originalFilesChanged": false,
                }))
            }
            Command::Albums {
                command: AlbumCommand::Add(args),
            } => {
                // The membership document was validated before connecting.
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Add, args, photo_ids, &client, admission).await
            }
            Command::Albums {
                command: AlbumCommand::Remove(args),
            } => {
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Remove, args, photo_ids, &client, admission)
                    .await
            }
            Command::Albums {
                command: AlbumCommand::Reorder(args),
            } => {
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Reorder, args, photo_ids, &client, admission)
                    .await
            }
            Command::Photos {
                command: PhotoCommand::List(args),
            } => {
                let data: ListData<PhotoListItem> = if let Some(cursor) = &args.cursor {
                    client
                        .json(
                            operation,
                            Method::GET,
                            client.endpoint(&["api", "photo-queries", cursor]),
                            None,
                        )
                        .await?
                } else {
                    let source = args
                        .album
                        .as_deref()
                        .map(|album_id| PhotoSource::Album { album_id })
                        .or_else(|| {
                            args.folder
                                .as_deref()
                                .map(|location| PhotoSource::Folder { location })
                        });
                    let request = PhotoQueryRequest {
                        source,
                        selection: args.selection,
                        rating_minimum: args.rating_min,
                        rating_maximum: args.rating_max,
                        kind: args.kind,
                        available: args.available,
                        captured_from: args.captured_from.as_deref(),
                        captured_before: args.captured_before.as_deref(),
                        order: args.order,
                        limit: args.limit,
                    };
                    let body = serde_json::to_value(request)
                        .map_err(|_| CommandFailure::transport(operation))?;
                    client
                        .json(
                            operation,
                            Method::POST,
                            client.endpoint(&["api", "photo-queries"]),
                            Some(body),
                        )
                        .await?
                };
                let page_limit = if args.cursor.is_some() {
                    MAXIMUM_LIST_PAGE
                } else {
                    usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
                };
                if !list_expiry_valid(&data, page_limit)
                    || data.next_cursor.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                let items = data
                    .items
                    .into_iter()
                    .map(|item| match item {
                        PhotoListItem::Missing(missing) => missing_value(missing),
                        PhotoListItem::Present(photo) => photo_value(photo, &client.origin),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| CommandFailure::transport(operation))?;
                Ok(json!({
                    "items": items,
                    "total": data.total,
                    "nextCursor": data.next_cursor,
                    "evaluatedAt": data.evaluated_at,
                    "expiresAt": data.expires_at,
                }))
            }
            Command::Photos {
                command: PhotoCommand::Get { photo_id },
            } => {
                let photo: PhotoGet = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", photo_id]),
                        None,
                    )
                    .await?;
                let PhotoGet {
                    id,
                    filename,
                    original_kind,
                    original_available,
                    selection_state,
                    rating,
                    decision_version,
                    removed_at_ms,
                    capture_time,
                    preview,
                    web_path,
                    metadata,
                } = photo;
                let metadata_values_absent = metadata.capture_time.is_none()
                    && metadata.aperture.is_none()
                    && metadata.shutter_speed.is_none()
                    && metadata.focal_length.is_none()
                    && metadata.iso.is_none();
                if metadata
                    .capture_time
                    .as_deref()
                    .is_some_and(|value| !valid_camera_time(value))
                    || (!matches!(metadata.state, MetadataState::Known) && !metadata_values_absent)
                {
                    return Err(CommandFailure::transport(operation));
                }
                let item = PhotoItem {
                    id,
                    filename,
                    original_kind,
                    original_available,
                    selection_state,
                    rating,
                    decision_version,
                    removed_at_ms,
                    capture_time,
                    preview,
                    web_path,
                };
                let mut value = photo_value(item, &client.origin)
                    .map_err(|_| CommandFailure::transport(operation))?;
                value
                    .as_object_mut()
                    .ok_or_else(|| CommandFailure::transport(operation))?
                    .insert(
                        "metadata".to_owned(),
                        serde_json::to_value(metadata)
                            .map_err(|_| CommandFailure::transport(operation))?,
                    );
                Ok(value)
            }
            Command::Photos {
                command: PhotoCommand::Preview { photo_id, size, .. },
            } => {
                preview_download::download(
                    &client,
                    photo_id,
                    *size,
                    preview_destination.expect("Preview destination was checked"),
                    publication,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::Metadata { photo_id },
            } => {
                let read: MetadataReadWire = client
                    .metadata_json(
                        operation,
                        client.endpoint(&["api", "photos", photo_id, "external-metadata"]),
                    )
                    .await?;
                if read.photo_id != *photo_id
                    || read.original_location.is_empty()
                    || read.evidence.photo_id != *photo_id
                    || read.evidence.instance_epoch.is_empty()
                {
                    return Err(CommandFailure::transport(operation));
                }
                serde_json::to_value(read).map_err(|_| CommandFailure::transport(operation))
            }
            Command::Photos {
                command: PhotoCommand::MetadataSave(args),
            } => {
                let document = pending_metadata_save
                    .as_ref()
                    .expect("metadata save input was prepared");
                let identity = MutationIdentity {
                    operation,
                    photo_ids: vec![args.photo_id.clone()],
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                let result: MetadataSaveResultWire = client
                    .metadata_mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "photos", &args.photo_id, "external-metadata"]),
                        document.clone(),
                    )
                    .await?;
                if result.photo_id != args.photo_id || result.sidecar_location.is_empty() {
                    return Err(CommandFailure::unknown(&identity));
                }
                serde_json::to_value(result).map_err(|_| CommandFailure::unknown(&identity))
            }
            Command::Photos {
                command: PhotoCommand::Set(args),
            } => {
                // The decision document was validated before connecting; the
                // single-Photo forms are a one-item batch of the same shape.
                let prepared = match pending_decision {
                    Some(prepared) => prepared,
                    None => single_photo_decision(args),
                };
                let identity = MutationIdentity {
                    operation,
                    photo_ids: prepared
                        .photos
                        .iter()
                        .map(|photo| photo.photo_id.clone())
                        .collect(),
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                let result: PhotoDecisionWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "photo-decisions"]),
                        prepared.body(),
                    )
                    .await?;
                confirmed_decision_result(&identity, &prepared, result)
            }
            Command::Photos {
                command: PhotoCommand::Remove(args),
            } => {
                let prepared = pending_removal
                    .as_ref()
                    .expect("removal input was prepared");
                let identity = MutationIdentity {
                    operation,
                    photo_ids: prepared
                        .photos
                        .iter()
                        .map(|photo| photo.photo_id.clone())
                        .collect(),
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                let result: PhotoRemovalWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "photos", "remove-explicit"]),
                        prepared.body(&args.operation_id),
                    )
                    .await?;
                confirmed_removal_result(&identity, &args.operation_id, prepared, result)
            }
            Command::Photos {
                command: PhotoCommand::RemovalOperation { operation_id },
            } => {
                let result: PhotoRemovalWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", "removal-operations", operation_id]),
                        None,
                    )
                    .await?;
                removal_wire_value(result, operation)
            }
            Command::Photos {
                command: PhotoCommand::RestoreOperation { operation_id },
            } => {
                let result: PhotoRestoreWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", "restore-operations", operation_id]),
                        None,
                    )
                    .await?;
                restore_wire_value(result, operation)
            }
            Command::Photos {
                command: PhotoCommand::Restore(args),
            } => {
                let photo_ids = pending_restore
                    .as_ref()
                    .map(|prepared| {
                        prepared
                            .markers
                            .iter()
                            .map(|marker| marker.photo_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let identity = MutationIdentity {
                    operation,
                    photo_ids,
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                match pending_restore.as_ref() {
                    Some(prepared) => {
                        let result: PhotoRestoreWire = client
                            .mutation(
                                &identity,
                                admission,
                                client.endpoint(&["api", "photos", "restore-explicit"]),
                                prepared.body(&args.operation_id),
                            )
                            .await?;
                        confirmed_restore_result(&identity, &args.operation_id, prepared, result)
                    }
                    None => {
                        let data: Value = client
                            .mutation(
                                &identity,
                                admission,
                                client.endpoint(&["api", "photos", "restore"]),
                                json!({ "operation": args.operation_id }),
                            )
                            .await?;
                        if !data.is_object() {
                            return Err(CommandFailure::unknown(&identity));
                        }
                        Ok(data)
                    }
                }
            }
            Command::Trash {
                command: TrashCommand::List(args),
            } => {
                let mut url = client.endpoint(&["api", "trash"]);
                url.query_pairs_mut()
                    .append_pair("start", &args.start.to_string())
                    .append_pair("limit", &args.limit.to_string());
                let data: Value = client.json(operation, Method::GET, url, None).await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
            }
            Command::Trash {
                command: TrashCommand::Review(args),
            } => {
                let pending = pending_trash_review
                    .as_ref()
                    .expect("Trash review input was prepared");
                let data: Value = client
                    .json(
                        operation,
                        Method::POST,
                        client.endpoint(&["api", "trash", "review"]),
                        Some(json!({
                            "operationId": args.operation_id,
                            "all": args.all,
                            "photoIds": pending.photo_ids,
                            "excludePhotoIds": pending.exclude_photo_ids,
                        })),
                    )
                    .await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
            }
            Command::Trash {
                command: TrashCommand::Delete { operation_id },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: None,
                    mappings: Vec::new(),
                };
                client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "trash", "delete"]),
                        json!({ "operationId": operation_id }),
                    )
                    .await
            }
            Command::Trash {
                command: TrashCommand::Operation { operation_id },
            } => {
                let data: Value = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "trash", "operations", operation_id]),
                        None,
                    )
                    .await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
            }
            Command::Recovery {
                command: RecoveryCommand::Unavailable(args),
            } => {
                if let Some(limit) = args.limit
                    && usize::from(limit) > limits.recovery_page_maximum
                {
                    return Err(CommandFailure::limit_exceeded(
                        "recoveryPageMaximum",
                        limits.recovery_page_maximum,
                        usize::from(limit),
                    ));
                }
                let data: ListData<RecoveryItemWire> = if let Some(cursor) = &args.cursor {
                    client
                        .json(
                            operation,
                            Method::GET,
                            client.endpoint(&["api", "recovery", "unavailable", cursor]),
                            None,
                        )
                        .await?
                } else {
                    let mut body = json!({});
                    if let Some(limit) = args.limit {
                        body["limit"] = json!(limit);
                    }
                    client
                        .json(
                            operation,
                            Method::POST,
                            client.endpoint(&["api", "recovery", "unavailable"]),
                            Some(body),
                        )
                        .await?
                };
                let page_limit = if args.cursor.is_some() {
                    limits.recovery_page_maximum
                } else {
                    args.limit.map_or(limits.recovery_page_maximum, usize::from)
                };
                if !list_expiry_valid(&data, page_limit)
                    || data.next_cursor.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                let items = data
                    .items
                    .into_iter()
                    .map(|item| recovery_item_value(item, &client.origin))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| CommandFailure::transport(operation))?;
                Ok(json!({
                    "items": items,
                    "total": data.total,
                    "nextCursor": data.next_cursor,
                    "evaluatedAt": data.evaluated_at,
                    "expiresAt": data.expires_at,
                }))
            }
            Command::Recovery {
                command: RecoveryCommand::Propose(args),
            } => {
                if let Some(limit) = args.limit
                    && usize::from(limit) > limits.recovery_page_maximum
                {
                    return Err(CommandFailure::limit_exceeded(
                        "recoveryPageMaximum",
                        limits.recovery_page_maximum,
                        usize::from(limit),
                    ));
                }
                let data: ListData<RecoveryMappingWire> = if let Some(cursor) = &args.cursor {
                    client
                        .json(
                            operation,
                            Method::GET,
                            client.endpoint(&["api", "recovery", "proposals", cursor]),
                            None,
                        )
                        .await?
                } else if let (Some(old_prefix), Some(new_prefix)) =
                    (&args.old_prefix, &args.new_prefix)
                {
                    let mut body = json!({
                        "oldPrefix": old_prefix,
                        "newPrefix": new_prefix,
                    });
                    if let Some(limit) = args.limit {
                        body["limit"] = json!(limit);
                    }
                    client
                        .json(
                            operation,
                            Method::POST,
                            client.endpoint(&["api", "recovery", "propose"]),
                            Some(body),
                        )
                        .await?
                } else {
                    client
                        .json(
                            operation,
                            Method::POST,
                            client.endpoint(&["api", "recovery", "propose"]),
                            Some(json!({
                                "originalId": args.original_id,
                                "newLocation": args.new_location,
                            })),
                        )
                        .await?
                };
                let page_limit = if args.cursor.is_some() {
                    limits.recovery_page_maximum
                } else {
                    args.limit.map_or(limits.recovery_page_maximum, usize::from)
                };
                if !list_expiry_valid(&data, page_limit)
                    || data.next_cursor.as_deref().is_some_and(str::is_empty)
                {
                    return Err(CommandFailure::transport(operation));
                }
                let single_form = args.original_id.is_some();
                if single_form && (data.total != 1 || data.next_cursor.is_some()) {
                    return Err(CommandFailure::transport(operation));
                }
                let items = data
                    .items
                    .into_iter()
                    .map(recovery_mapping_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| CommandFailure::transport(operation))?;
                Ok(json!({
                    "items": items,
                    "total": data.total,
                    "nextCursor": data.next_cursor,
                    "evaluatedAt": data.evaluated_at,
                    "expiresAt": data.expires_at,
                }))
            }
            Command::Recovery {
                command: RecoveryCommand::Apply(_args),
            } => {
                let prepared = pending_recovery_apply
                    .as_ref()
                    .expect("recovery apply input was prepared");
                if prepared.identities.len() > limits.recovery_apply_maximum {
                    return Err(CommandFailure::limit_exceeded(
                        "recoveryApplyMaximum",
                        limits.recovery_apply_maximum,
                        prepared.identities.len(),
                    ));
                }
                let identity = MutationIdentity {
                    mappings: prepared.identities.clone(),
                    ..MutationIdentity::bare(operation)
                };
                let result: RecoveryApplyData = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "recovery", "apply"]),
                        prepared.body.clone(),
                    )
                    .await?;
                recovery_apply_value(&identity, result, prepared, &client.origin)
            }
        }
    }
    .await;
    match result {
        Ok(mut data) => {
            redact_value(&mut data, &client.token);
            Ok(data)
        }
        Err(mut failure) => {
            redact_error(&mut failure.payload, &client.token);
            // Attached failure data is server-controlled on the same
            // untrusted path as the payload.
            if let Some(data) = failure.data.as_mut() {
                redact_value(data, &client.token);
            }
            Err(failure)
        }
    }
}

pub(crate) fn redact_error(error: &mut ErrorPayload, secret: &str) {
    redact_string(&mut error.code, secret);
    redact_string(&mut error.message, secret);
    redact_string(&mut error.effect, secret);
    redact_value(&mut error.details, secret);
}

pub(crate) fn redact_value(value: &mut Value, secret: &str) {
    match value {
        Value::String(string) => redact_string(string, secret),
        Value::Array(items) => {
            for item in items {
                redact_value(item, secret);
            }
        }
        Value::Object(fields) => {
            let previous = std::mem::take(fields);
            for (mut key, mut field) in previous {
                redact_string(&mut key, secret);
                redact_value(&mut field, secret);
                fields.insert(key, field);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub(crate) fn redact_string(value: &mut String, secret: &str) {
    if !secret.is_empty() && value.contains(secret) {
        *value = value.replace(secret, "[redacted]");
    }
}

pub(crate) fn validate_command(command: &Command) -> Result<(), CommandFailure> {
    match command {
        Command::Photos {
            command: PhotoCommand::List(args),
        } if args.cursor.is_none() => {
            if args
                .rating_min
                .zip(args.rating_max)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(CommandFailure::invalid(
                    "rating-min",
                    "The minimum Rating must not exceed the maximum Rating.",
                ));
            }
            if args
                .captured_from
                .as_ref()
                .zip(args.captured_before.as_ref())
                .is_some_and(|(from, before)| from >= before)
            {
                return Err(CommandFailure::invalid(
                    "captured-from",
                    "The lower Capture Time bound must precede the upper bound.",
                ));
            }
            if args.order == Some(OrderArg::AlbumOrder) && args.album.is_none() {
                return Err(CommandFailure::invalid(
                    "order",
                    "album-order requires an Album source.",
                ));
            }
            Ok(())
        }
        Command::Photos {
            command: PhotoCommand::Set(args),
        } if args.input.is_none() => {
            if args.photo_id.is_none() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "photos set needs PHOTO_ID with --selection or --rating and --if-version, or --input FILE.",
                ));
            }
            if args.selection.is_none() && args.rating.is_none() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "The single-Photo forms need exactly one of --selection or --rating.",
                ));
            }
            if args.if_version.is_none() {
                return Err(CommandFailure::invalid(
                    "if-version",
                    "The single-Photo forms need the decision version observed by a prior read.",
                ));
            }
            Ok(())
        }
        Command::Photos {
            command:
                PhotoCommand::ProcessingExportStatus { request_id, .. }
                | PhotoCommand::ProcessingExportCancel { request_id, .. }
                | PhotoCommand::ProcessingExportRetry { request_id, .. },
        } => {
            if !valid_request_identity(request_id) {
                return Err(CommandFailure::invalid(
                    "request-id",
                    "The request identity must be 1 through 128 characters of ASCII letters, digits, '.', '_', or '-'.",
                ));
            }
            Ok(())
        }
        Command::Trash {
            command: TrashCommand::Review(args),
        } => {
            if args.all == args.input.is_some() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "trash review needs exactly one of --all or --input.",
                ));
            }
            if args.exclude_input.is_some() && !args.all {
                return Err(CommandFailure::invalid(
                    "exclude-input",
                    "--exclude-input requires --all.",
                ));
            }
            Ok(())
        }
        Command::Recovery {
            command: RecoveryCommand::Propose(args),
        } if args.cursor.is_none() => {
            let prefix_form = args.old_prefix.is_some() || args.new_prefix.is_some();
            let single_form = args.original_id.is_some() || args.new_location.is_some();
            if prefix_form && single_form {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "recovery propose takes either --old-prefix with --new-prefix, or --original-id with --new-location.",
                ));
            }
            if !prefix_form && !single_form {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "recovery propose needs --old-prefix and --new-prefix, or --original-id and --new-location, or --cursor.",
                ));
            }
            if prefix_form {
                let (Some(old_prefix), Some(new_prefix)) = (&args.old_prefix, &args.new_prefix)
                else {
                    return Err(CommandFailure::invalid(
                        "old-prefix",
                        "The Folder-prefix form needs both --old-prefix and --new-prefix.",
                    ));
                };
                for (argument, value) in [("old-prefix", old_prefix), ("new-prefix", new_prefix)] {
                    if !valid_location_prefix(value) {
                        return Err(CommandFailure::invalid(
                            argument,
                            "A prefix is a Library-relative Folder Location with no leading or trailing separator.",
                        ));
                    }
                }
            } else {
                let Some(original_id) = &args.original_id else {
                    return Err(CommandFailure::invalid(
                        "original-id",
                        "The single-mapping form needs both --original-id and --new-location.",
                    ));
                };
                let Some(new_location) = &args.new_location else {
                    return Err(CommandFailure::invalid(
                        "original-id",
                        "The single-mapping form needs both --original-id and --new-location.",
                    ));
                };
                if !valid_library_id(original_id) {
                    return Err(CommandFailure::invalid(
                        "original-id",
                        "The Original id is not a Library identity.",
                    ));
                }
                if !valid_original_location(new_location) {
                    return Err(CommandFailure::invalid(
                        "new-location",
                        "The Location is a Library-relative Original Location including the filename.",
                    ));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn access_token_path(cli: &Cli) -> Result<PathBuf, CommandFailure> {
    let path = cli
        .token_file
        .clone()
        .or_else(|| env::var_os("SLIPSTREAM_ACCESS_TOKEN_FILE").map(PathBuf::from));
    let Some(path) = path else {
        return Err(CommandFailure::invalid(
            "token-file",
            "Set --token-file or SLIPSTREAM_ACCESS_TOKEN_FILE.",
        ));
    };
    if path.as_os_str().is_empty() {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file path must not be empty.",
        ));
    }
    Ok(path)
}

pub(crate) async fn read_access_token(path: PathBuf) -> Result<String, CommandFailure> {
    tokio::task::spawn_blocking(move || read_access_token_file(&path))
        .await
        .map_err(|_| CommandFailure::local_credential(None))?
}

#[cfg(unix)]
pub(crate) fn read_access_token_file(path: &Path) -> Result<String, CommandFailure> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    const MAXIMUM_CREDENTIAL_BYTES: usize = 45;
    let path_text = path.to_str();
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            CommandFailure::invalid(
                "token-file",
                "The credential file must be a nonsymlink regular file.",
            )
        } else {
            CommandFailure::local_credential(path_text)
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| CommandFailure::local_credential(path_text))?;
    // Check the file opened above, so a symlink swap cannot change which file
    // is validated and read. Do not disclose which credential rule failed.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAXIMUM_CREDENTIAL_BYTES as u64
    {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must be a private regular file owned by the current user.",
        ));
    }
    let mut bytes = Vec::with_capacity(MAXIMUM_CREDENTIAL_BYTES);
    file.take((MAXIMUM_CREDENTIAL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CommandFailure::local_credential(path_text))?;
    if bytes.len() > MAXIMUM_CREDENTIAL_BYTES {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must contain one Access Token and an optional line ending.",
        ));
    }
    let token_bytes = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(&bytes);
    if !canonical_access_token(token_bytes) {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must contain one canonical Access Token and an optional line ending.",
        ));
    }
    // canonical_access_token accepts only ASCII base64url bytes.
    Ok(String::from_utf8(token_bytes.to_vec()).expect("validated token is ASCII"))
}

#[cfg(not(unix))]
pub(crate) fn read_access_token_file(_path: &Path) -> Result<String, CommandFailure> {
    Err(CommandFailure::invalid(
        "token-file",
        "Private credential-file checks are unavailable on this platform.",
    ))
}

pub(crate) fn canonical_access_token(token: &[u8]) -> bool {
    token.len() == 43
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && matches!(
            token[42],
            b'A' | b'E'
                | b'I'
                | b'M'
                | b'Q'
                | b'U'
                | b'Y'
                | b'c'
                | b'g'
                | b'k'
                | b'o'
                | b's'
                | b'w'
                | b'0'
                | b'4'
                | b'8'
        )
}
mod invocation;
#[cfg(test)]
pub(crate) use invocation::{deadline_failure, render_text};
pub use invocation::{invalid_invocation, invoke, invoke_until};
