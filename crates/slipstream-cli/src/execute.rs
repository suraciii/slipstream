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
            command:
                PhotoCommand::Export {
                    command: PhotoExportCommand::Download(args),
                },
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::Export,
            &args.file,
        )?),
        Command::Photos {
            command: PhotoCommand::EditPreview(args),
        } => Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::EditPreview,
            &args.file,
        )?),
        _ => None,
    };
    let origin = service_origin(cli, environment)?;
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
    let pending_recipe = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Recipe { command },
        } => development::prepare(command).await?,
        _ => None,
    };
    let client = ServiceClient::new(origin, token)?;
    client.capabilities(operation).await?;

    let result = async {
        match &cli.command {
            Command::Processing { .. } => development::capability(&client).await,
            Command::Photos {
                command: PhotoCommand::Recipe { command },
            } => development::execute(&client, admission, command, pending_recipe).await,
            Command::Photos {
                command: PhotoCommand::EditPreview(args),
            } => {
                edit_preview_download::download(
                    &client,
                    args,
                    preview_destination.expect("Edit Preview destination was checked"),
                    publication,
                )
                .await
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
            Command::Photos {
                command:
                    PhotoCommand::Export {
                        command: PhotoExportCommand::Submit(args),
                    },
            } => export_submission(&client, admission, args, operation).await,
            Command::Photos {
                command:
                    PhotoCommand::Export {
                        command: PhotoExportCommand::List { photo_id },
                    },
            } => {
                let data: ExportListWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", photo_id, "exports"]),
                        None,
                    )
                    .await?;
                export_list_value(data, operation)
            }
            Command::Photos {
                command:
                    PhotoCommand::Export {
                        command: PhotoExportCommand::Status { export_id },
                    },
            } => {
                let data: ExportInspectWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "exports", export_id]),
                        None,
                    )
                    .await?;
                let inspected = validated_export_inspect(data, export_id, operation)?;
                serde_json::to_value(inspected).map_err(|_| CommandFailure::transport(operation))
            }
            Command::Photos {
                command:
                    PhotoCommand::Export {
                        command: PhotoExportCommand::Download(args),
                    },
            } => {
                export_download::download(
                    &client,
                    &args.export_id,
                    preview_destination.expect("Export destination was checked"),
                    publication,
                )
                .await
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
                PhotoCommand::Export {
                    command: PhotoExportCommand::Submit(args),
                },
        } => {
            if !valid_request_identity(&args.request_id) {
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

pub async fn invoke(cli: Cli, environment: Option<&str>) -> InvocationResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.timeout);
    invoke_until(cli, environment, deadline).await
}

pub async fn invoke_until(
    cli: Cli,
    environment: Option<&str>,
    deadline: tokio::time::Instant,
) -> InvocationResult {
    let output = cli.output;
    let operation = command_operation(&cli.command);
    let admission = AdmissionState::default();
    let publication = PublicationState::default();
    let command = tokio::time::timeout_at(
        deadline,
        execute(&cli, environment, &admission, &publication),
    );
    tokio::pin!(command);
    let (exit_code, envelope) = tokio::select! {
        result = &mut command => match result {
            Ok(Ok(data)) => (0, Envelope::success(data)),
            Ok(Err(failure)) => {
                let envelope = match failure.data {
                    Some(data) if failure.payload.effect == "partial" => {
                        Envelope::partial(*data, failure.payload)
                    }
                    Some(data) => Envelope::error_with_data(*data, failure.payload),
                    None => Envelope::error(failure.payload),
                };
                (failure.exit_code, envelope)
            }
            Err(_) => {
                let failure = match publication.committed() {
                    Some(data) => {
                        CommandFailure::published_file(data, false, publication.committed_noun())
                    }
                    None => match admission.admitted() {
                        Some(identity) => CommandFailure::unknown(&identity),
                        None => CommandFailure::transport(operation),
                    },
                };
                let envelope = match failure.data {
                    Some(data) => Envelope::partial(*data, failure.payload),
                    None => Envelope::error(failure.payload),
                };
                (failure.exit_code, envelope)
            }
        },
        _ = tokio::signal::ctrl_c() => {
            if let Some(data) = publication.committed() {
                let failure =
                    CommandFailure::published_file(data, true, publication.committed_noun());
                (130, Envelope::partial(*failure.data.unwrap(), failure.payload))
            } else {
                let failure = match admission.admitted() {
                Some(identity) => CommandFailure::interrupted_unknown(&identity),
                None => {
                    let mut failure = CommandFailure::transport(operation);
                    failure.payload.message = "The command was interrupted. Inspect status before continuing.".to_owned();
                    failure
                }
            };
            // A handled interruption exits 130 whether or not a request may
            // have been admitted; only the envelope distinguishes the cases.
            (130, Envelope::error(failure.payload))
            }
        }
    };
    render_invocation(
        output,
        exit_code,
        &envelope,
        publication
            .committed()
            .and_then(|value| value["path"].as_str().map(str::to_owned)),
    )
}

pub fn invalid_invocation(output: OutputFormat, reason: impl Into<String>) -> InvocationResult {
    let failure = CommandFailure::invalid("arguments", reason);
    render_invocation(
        output,
        failure.exit_code,
        &Envelope::error(failure.payload),
        None,
    )
}

pub(crate) fn render_invocation(
    output: OutputFormat,
    exit_code: u8,
    envelope: &Envelope,
    committed_preview_path: Option<String>,
) -> InvocationResult {
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string(envelope).expect("envelope serialization is infallible")
        ),
        OutputFormat::Text => render_text(envelope),
    };
    InvocationResult {
        exit_code,
        stdout,
        committed_preview_path,
    }
}

pub(crate) fn render_text(envelope: &Envelope) -> String {
    match (&envelope.data, &envelope.error) {
        (Some(data), None) => format!(
            "Success\n{}\n",
            serde_json::to_string_pretty(data).expect("result serialization is infallible")
        ),
        (_, Some(error)) => format!(
            "Error: {}\n{}\n{}\n",
            error.code,
            error.message,
            serde_json::to_string_pretty(&error.details)
                .expect("error serialization is infallible")
        ),
        _ => "Error: invalid result\n".to_owned(),
    }
}
