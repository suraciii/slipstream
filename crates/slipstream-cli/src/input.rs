use super::*;

pub(crate) fn service_origin(cli: &Cli, environment: Option<&str>) -> Result<Url, CommandFailure> {
    let value = cli.server.as_deref().or(environment).ok_or_else(|| {
        CommandFailure::invalid(
            "server",
            "Set --server or SLIPSTREAM_SERVER_URL to an HTTP or HTTPS service origin.",
        )
    })?;
    if value.is_empty() {
        return Err(CommandFailure::invalid(
            "server",
            "The service URL must not be empty.",
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        CommandFailure::invalid("server", "The service URL must be an HTTP or HTTPS origin.")
    })?;
    let has_userinfo = value
        .split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or(rest)
                .contains('@')
        })
        .unwrap_or(false);
    let valid = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && !has_userinfo
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    if !valid {
        return Err(CommandFailure::invalid(
            "server",
            "The service URL must be an HTTP or HTTPS origin without credentials, path, query, or fragment.",
        ));
    }
    Ok(url)
}

pub(crate) fn web_url(origin: &Url, path: &str) -> Result<String, ()> {
    if !path.starts_with("/?") || path.contains('#') {
        return Err(());
    }
    let resolved = origin.join(path).map_err(|_| ())?;
    if resolved.scheme() != origin.scheme()
        || resolved.host_str() != origin.host_str()
        || resolved.port_or_known_default() != origin.port_or_known_default()
    {
        return Err(());
    }
    Ok(resolved.to_string())
}

pub(crate) fn validate_nonempty(value: &str) -> Result<(), ()> {
    (!value.is_empty()).then_some(()).ok_or(())
}

pub(crate) fn valid_camera_time(value: &str) -> bool {
    if !value.is_ascii() || value.len() < 19 || local_time(&value[..19]).is_err() {
        return false;
    }
    value.len() == 19
        || value
            .strip_prefix(&value[..19])
            .and_then(|fraction| fraction.strip_prefix('.'))
            .is_some_and(|fraction| {
                (1..=9).contains(&fraction.len())
                    && fraction.bytes().all(|byte| byte.is_ascii_digit())
            })
}

pub(crate) fn valid_utc_time(value: &str) -> bool {
    value.strip_suffix('Z').is_some_and(valid_camera_time)
}

pub(crate) fn missing_value(missing: MissingItem) -> Result<Value, ()> {
    validate_nonempty(&missing.id)?;
    serde_json::to_value(missing).map_err(|_| ())
}

pub(crate) fn album_value(album: AlbumSummary, origin: &Url) -> Result<Value, ()> {
    validate_nonempty(&album.id)?;
    validate_nonempty(&album.album_version)?;
    Ok(json!({
        "id": album.id,
        "name": album.name,
        "photoCount": album.photo_count,
        "hasSavedPosition": album.has_saved_position,
        "albumVersion": album.album_version,
        "webUrl": web_url(origin, &album.web_path)?,
    }))
}

/// Reads the bounded UTF-8 bytes of one `--input` document. The blocking-pool
/// read is not itself bounded; on deadline or interruption the
/// executable-boundary terminal exit publishes the envelope and abandons a
/// still-blocked read.
pub(crate) async fn read_input_bytes(input: &str) -> Result<Vec<u8>, CommandFailure> {
    let owned = input.to_owned();
    tokio::task::spawn_blocking(move || {
        if owned == "-" {
            let mut stdin = std::io::stdin().lock();
            read_bounded(&mut stdin, None)
        } else {
            let mut file = std::fs::File::open(&owned)
                .map_err(|_| CommandFailure::local_input(Some(&owned)))?;
            read_bounded(&mut file, Some(&owned))
        }
    })
    .await
    .map_err(|_| CommandFailure::local_input(None))?
}

pub(crate) fn read_bounded(
    source: &mut dyn std::io::Read,
    path: Option<&str>,
) -> Result<Vec<u8>, CommandFailure> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|_| CommandFailure::local_input(path))?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > MAXIMUM_INPUT_BYTES {
            return Err(CommandFailure::limit_exceeded(
                "inputBytesMaximum",
                MAXIMUM_INPUT_BYTES,
                MAXIMUM_INPUT_BYTES + 1,
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

/// Validates the complete membership document before any write is attempted.
pub(crate) fn parse_membership_ids(
    bytes: Vec<u8>,
    limit_name: &'static str,
) -> Result<Vec<String>, CommandFailure> {
    let document: MembershipInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with only an ordered photoIds array.",
        )
    })?;
    let photo_ids = document.photo_ids;
    if photo_ids.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            limit_name,
            MAXIMUM_MUTATION_PHOTO_IDS,
            photo_ids.len(),
        ));
    }
    if photo_ids.is_empty() {
        return Err(CommandFailure::invalid(
            "input",
            "The photoIds array must contain at least one Photo ID.",
        ));
    }
    if photo_ids.iter().any(|photo_id| photo_id.is_empty()) {
        return Err(CommandFailure::invalid(
            "input",
            "Each Photo ID must be a nonempty string.",
        ));
    }
    let distinct = photo_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if distinct != photo_ids.len() {
        return Err(CommandFailure::invalid(
            "input",
            "The photoIds array must not repeat a Photo ID.",
        ));
    }
    Ok(photo_ids)
}

pub(crate) async fn read_membership_ids(
    input: &str,
    limit_name: &'static str,
) -> Result<Vec<String>, CommandFailure> {
    parse_membership_ids(read_input_bytes(input).await?, limit_name)
}

/// Reads and structurally validates one complete metadata save document
/// before any network access. Field semantics stay server-owned; this
/// catches shape errors locally and enforces that the evidence names the
/// same Photo as the command.
pub(crate) async fn prepare_metadata_save(
    photo_id: &str,
    input: &str,
) -> Result<Value, CommandFailure> {
    let bytes = read_input_bytes(input).await?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|_| CommandFailure::invalid("input", "The save document is not valid JSON."))?;
    let invalid = |reason: &str| CommandFailure::invalid("input", reason);
    let object = document
        .as_object()
        .ok_or_else(|| invalid("The save document must be a JSON object."))?;
    if object.len() != 2 || !object.contains_key("evidence") || !object.contains_key("changes") {
        return Err(invalid(
            "The save document must contain exactly \"evidence\" and \"changes\".",
        ));
    }
    let evidence_photo_id = object["evidence"]
        .as_object()
        .and_then(|evidence| evidence.get("photoId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            invalid("The evidence must name a non-empty photoId for the observed Read.")
        })?;
    if evidence_photo_id != photo_id {
        return Err(invalid(
            "The evidence photoId must match the Photo named by the command.",
        ));
    }
    let changes = object["changes"]
        .as_object()
        .filter(|changes| !changes.is_empty())
        .ok_or_else(|| invalid("The changes must be a non-empty JSON object."))?;
    for (field, change) in changes {
        if field.is_empty() {
            return Err(invalid("Every change must name a non-empty field."));
        }
        let change = change
            .as_object()
            .ok_or_else(|| invalid("Every change must be a JSON object."))?;
        let operation = change
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Every change must name an \"op\"."))?;
        match operation {
            "set" => {
                if change.len() != 2 || !change.contains_key("value") {
                    return Err(invalid(
                        "A \"set\" change must contain exactly \"op\" and \"value\".",
                    ));
                }
            }
            "clear" | "remove" => {
                if change.len() != 1 {
                    return Err(invalid(
                        "A \"clear\" or \"remove\" change must contain only \"op\".",
                    ));
                }
            }
            "setLanguages" => {
                let languages = change
                    .get("languages")
                    .and_then(Value::as_object)
                    .filter(|languages| !languages.is_empty())
                    .ok_or_else(|| {
                        invalid(
                            "A \"setLanguages\" change must name a non-empty \"languages\" map.",
                        )
                    })?;
                if change.len() != 2 {
                    return Err(invalid(
                        "A \"setLanguages\" change must contain exactly \"op\" and \"languages\".",
                    ));
                }
                for (language, value) in languages {
                    if language.is_empty() || !value.is_null() && !value.is_string() {
                        return Err(invalid(
                            "Every language must map to text or null for removal.",
                        ));
                    }
                }
            }
            _ => {
                return Err(invalid(
                    "Every \"op\" must be set, clear, remove, or setLanguages.",
                ));
            }
        }
    }
    Ok(document)
}

pub(crate) async fn read_trash_ids(input: &str) -> Result<Vec<String>, CommandFailure> {
    let bytes = read_input_bytes(input).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be a JSON object with photoIds or a bare ID array.",
        )
    })?;
    let photo_ids = match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>(),
        Value::Object(mut object) => {
            let value = object.remove("photoIds");
            if !object.is_empty() {
                None
            } else {
                value.and_then(|value| {
                    value
                        .as_array()?
                        .iter()
                        .map(|value| value.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
            }
        }
        _ => None,
    }
    .ok_or_else(|| {
        CommandFailure::invalid(
            "input",
            "The Trash ID input must contain only a photoIds string array.",
        )
    })?;
    if photo_ids.len() > MAXIMUM_TRASH_IDS {
        return Err(CommandFailure::limit_exceeded(
            "permanentDeletionPhotoIdsMaximum",
            MAXIMUM_TRASH_IDS,
            photo_ids.len(),
        ));
    }
    if photo_ids.iter().any(String::is_empty)
        || photo_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != photo_ids.len()
    {
        return Err(CommandFailure::invalid(
            "input",
            "Trash Photo IDs must be nonempty and distinct.",
        ));
    }
    Ok(photo_ids)
}

/// Validates the complete decision document before any write is attempted.
/// The refusal order mirrors the service's own admission order, so the same
/// request is refused for the same reason whichever boundary sees it first.
pub(crate) fn parse_decision_input(bytes: Vec<u8>) -> Result<PreparedDecision, CommandFailure> {
    let document: DecisionInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly field, value, and photos.",
        )
    })?;
    let field = match document.field.as_str() {
        "selectionState" => DecisionField::SelectionState,
        "rating" => DecisionField::Rating,
        _ => {
            return Err(CommandFailure::invalid(
                "field",
                "The decision field must be selectionState or rating.",
            ));
        }
    };

    let value_matches_field = match field {
        DecisionField::SelectionState => document
            .value
            .as_str()
            .is_some_and(|value| matches!(value, "unflagged" | "picked" | "rejected")),
        DecisionField::Rating => document.value.as_u64().is_some_and(|rating| rating <= 5),
    };
    if !value_matches_field {
        return Err(CommandFailure::invalid(
            "value",
            "The decision value must match the field's type and range.",
        ));
    }
    if document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            "photoIds",
            MAXIMUM_MUTATION_PHOTO_IDS,
            document.photos.len(),
        ));
    }
    if document.photos.is_empty()
        || document
            .photos
            .iter()
            .any(|photo| photo.photo_id.is_empty() || photo.if_version.is_empty())
        || document
            .photos
            .iter()
            .map(|photo| photo.photo_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != document.photos.len()
    {
        return Err(CommandFailure::invalid(
            "photos",
            "Photo items must be a nonempty ordered list of distinct Photo IDs with nonempty versions.",
        ));
    }
    Ok(PreparedDecision {
        field,
        value: document.value,
        photos: document
            .photos
            .into_iter()
            .map(|photo| DecisionTarget {
                photo_id: photo.photo_id,
                if_version: photo.if_version,
            })
            .collect(),
    })
}

pub(crate) async fn read_decision_input(input: &str) -> Result<PreparedDecision, CommandFailure> {
    parse_decision_input(read_input_bytes(input).await?)
}
pub(crate) fn parse_removal_input(bytes: Vec<u8>) -> Result<PreparedRemoval, CommandFailure> {
    let document: RemovalInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one object with a distinct photos evidence array.",
        )
    })?;
    if document.photos.is_empty() {
        return Err(CommandFailure::invalid(
            "photos",
            "The explicit removal target list must not be empty.",
        ));
    }
    if document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            "removalPhotoIdsMaximum",
            MAXIMUM_MUTATION_PHOTO_IDS,
            document.photos.len(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut photos = Vec::with_capacity(document.photos.len());
    for photo in document.photos {
        if photo.photo_id.is_empty()
            || !ids.insert(photo.photo_id.clone())
            || photo.selection_state != "rejected"
            || photo.decision_version.is_empty()
        {
            return Err(CommandFailure::invalid(
                "photos",
                "Each target must have a distinct ID, rejected Selection State, and nonempty decision version.",
            ));
        }
        let removed_at_ms = match photo.removed_at_ms {
            Value::Null => None,
            Value::Number(value) => match value.as_i64().filter(|value| *value >= 0) {
                Some(value) => Some(value),
                None => {
                    return Err(CommandFailure::invalid(
                        "removedAtMs",
                        "The removal marker must be null or nonnegative.",
                    ));
                }
            },
            _ => {
                return Err(CommandFailure::invalid(
                    "removedAtMs",
                    "The removal marker must be null or nonnegative.",
                ));
            }
        };
        photos.push(RemovalTarget {
            photo_id: photo.photo_id,
            selection_state: photo.selection_state,
            decision_version: photo.decision_version,
            removed_at_ms,
        });
    }
    Ok(PreparedRemoval { photos })
}

pub(crate) async fn read_removal_input(input: &str) -> Result<PreparedRemoval, CommandFailure> {
    parse_removal_input(read_input_bytes(input).await?)
}
pub(crate) fn parse_restore_input(bytes: Vec<u8>) -> Result<PreparedRestore, CommandFailure> {
    let document: RestoreInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one object with a distinct photos marker array.",
        )
    })?;
    if document.photos.is_empty() || document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(if document.photos.is_empty() {
            CommandFailure::invalid(
                "photos",
                "The explicit Restore target list must not be empty.",
            )
        } else {
            CommandFailure::limit_exceeded(
                "removalPhotoIdsMaximum",
                MAXIMUM_MUTATION_PHOTO_IDS,
                document.photos.len(),
            )
        });
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut markers = Vec::with_capacity(document.photos.len());
    for photo in document.photos {
        if photo.photo_id.is_empty()
            || !ids.insert(photo.photo_id.clone())
            || photo.removed_at_ms < 0
        {
            return Err(CommandFailure::invalid(
                "photos",
                "Restore markers must have distinct IDs and nonnegative removal identities.",
            ));
        }
        markers.push(RestoreMarker {
            photo_id: photo.photo_id,
            removed_at_ms: photo.removed_at_ms,
        });
    }
    Ok(PreparedRestore { markers })
}

pub(crate) async fn read_restore_input(input: &str) -> Result<PreparedRestore, CommandFailure> {
    parse_restore_input(read_input_bytes(input).await?)
}

/// Reads and completely validates one reviewed recovery apply document
/// before any network access. Every shape rule the service enforces on the
/// closed batch is checked locally, so an invalid batch can never depend on
/// service reachability.
pub(crate) async fn read_recovery_apply(
    input: &str,
) -> Result<PreparedRecoveryApply, CommandFailure> {
    let bytes = read_input_bytes(input).await?;
    let invalid = |reason: &'static str| CommandFailure::invalid("input", reason);
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("The apply document is not valid JSON."))?;
    let Some(mappings) = document
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("mappings"))
        .and_then(Value::as_array)
    else {
        return Err(invalid(
            "The apply document must be one JSON object with only a mappings array.",
        ));
    };
    if mappings.is_empty() {
        return Err(invalid(
            "The mappings array must contain at least one reviewed mapping.",
        ));
    }
    if mappings.len() > MAXIMUM_RECOVERY_APPLY {
        return Err(CommandFailure::limit_exceeded(
            "recoveryApplyMaximum",
            MAXIMUM_RECOVERY_APPLY,
            mappings.len(),
        ));
    }
    let mut body = Vec::with_capacity(mappings.len());
    let mut identities = Vec::with_capacity(mappings.len());
    let mut original_ids = std::collections::HashSet::new();
    let mut locations = std::collections::HashSet::new();
    for mapping in mappings {
        let Some(object) = mapping.as_object() else {
            return Err(invalid("Every mapping must be a JSON object."));
        };
        if !object.keys().all(|key| {
            matches!(
                key.as_str(),
                "originalId"
                    | "newLocation"
                    | "mappingId"
                    | "confirmUnverifiedContent"
                    | "retirePhotoId"
            )
        }) {
            return Err(invalid("A mapping contains an unknown key."));
        }
        let field = |name: &str| object.get(name).and_then(Value::as_str);
        let (Some(original_id), Some(new_location), Some(mapping_id)) = (
            field("originalId"),
            field("newLocation"),
            field("mappingId"),
        ) else {
            return Err(invalid(
                "Every mapping names its Original, Location, and reviewed identity.",
            ));
        };
        if !valid_library_id(original_id) {
            return Err(invalid(
                "A mapping names an Original id that is not a Library identity.",
            ));
        }
        if !valid_original_location(new_location) {
            return Err(invalid(
                "A mapping names a Location that is not a Library-relative Original Location.",
            ));
        }
        if mapping_id.is_empty() {
            return Err(invalid(
                "Every mapping carries the mappingId of its reviewed proposal.",
            ));
        }
        let confirm_unverified_content = match object.get("confirmUnverifiedContent") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => {
                return Err(invalid(
                    "confirmUnverifiedContent must be a boolean acknowledgement.",
                ));
            }
        };
        let retire_photo_id = match object.get("retirePhotoId") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if valid_library_id(value) => Some(value.clone()),
            Some(_) => {
                return Err(invalid(
                    "retirePhotoId must name one Photo identity of the reviewed retire candidate.",
                ));
            }
        };
        if !original_ids.insert(original_id.to_owned()) {
            return Err(invalid("One Original appears in more than one mapping."));
        }
        if !locations.insert(new_location.to_owned()) {
            return Err(invalid(
                "One destination Location appears in more than one mapping.",
            ));
        }
        let mut item = json!({
            "originalId": original_id,
            "newLocation": new_location,
            "mappingId": mapping_id,
        });
        if let Some(value) = confirm_unverified_content {
            item["confirmUnverifiedContent"] = json!(value);
        }
        if let Some(value) = retire_photo_id {
            item["retirePhotoId"] = json!(value);
        }
        identities.push(json!({
            "originalId": original_id,
            "newLocation": new_location,
            "mappingId": mapping_id,
        }));
        body.push(item);
    }
    Ok(PreparedRecoveryApply {
        body: json!({ "mappings": body }),
        identities,
    })
}

/// Builds the one-item batch shared by the single-Photo forms. The command
/// was semantically validated before any network access, so each required
/// piece is present.
pub(crate) fn single_photo_decision(args: &PhotoDecisionArgs) -> PreparedDecision {
    let (field, value) = if let Some(selection) = args.selection {
        (
            DecisionField::SelectionState,
            serde_json::to_value(selection).expect("selection values serialize"),
        )
    } else {
        (
            DecisionField::Rating,
            json!(args.rating.expect("one decision field is present")),
        )
    };
    PreparedDecision {
        field,
        value,
        photos: vec![DecisionTarget {
            photo_id: args.photo_id.clone().expect("a Photo ID is present"),
            if_version: args.if_version.clone().expect("a version is present"),
        }],
    }
}
