//! Bounded admission of one production Photo Start request: the checks
//! that refuse before any worker release, the staged source copy into the
//! launcher-owned snapshot, and the durable plan the verified copy admits
//! (`design/processing-photo-protocol.md`, admission ordering).

use super::photo_host::Headroom;
use super::*;

/// The outcome of the bounded admission checks of one Start request.
#[derive(Debug)]
pub(super) enum StartAdmission {
    /// A durable receipt for this attempt identity already exists.
    Replay(Box<ResultBody>),
    /// A fresh executor intent was persisted for the copied descriptor.
    Intent(Box<PhotoRecord>),
}

/// Perform every bounded admission check and persist the executor intent.
/// The descriptor copy runs afterwards, without the journal mutex held.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_start(
    registry: &mut Registry,
    config: &Config,
    root: &Path,
    request: &Request,
    descriptor: Option<&File>,
    image_id: &str,
    available: bool,
    headroom: &Headroom,
) -> Result<StartAdmission, ErrorCode> {
    let Request::Start {
        export_id,
        incarnation,
        sequence,
        policy,
        bundle,
        source,
        recipe,
        workload,
        recipe_digest: request_recipe_digest,
        manifest_sha256,
        ..
    } = request
    else {
        return Err(ErrorCode::InvalidRequest);
    };
    if incarnation != &registry.incarnation {
        return Err(ErrorCode::StaleIncarnation);
    }
    expire(registry, now()?, config.receipt_retention_seconds)?;
    if let Some(record) = registry.records.get(sequence) {
        // Replaying the same identity and digest resolves to the same
        // attempt; any conflicting identity data is a conflict. The
        // recomputed digest binds the declared source facts, including the
        // profile, so a replay can never diverge from the durable attempt.
        if record.export_id != *export_id
            || record.manifest_sha256 != *manifest_sha256
            || manifest_digest(request)? != record.manifest_sha256
        {
            return Err(ErrorCode::Conflict);
        }
        return Ok(StartAdmission::Replay(Box::new(
            record.result_body(incarnation),
        )));
    }
    if *sequence <= registry.watermark {
        return Err(ErrorCode::Expired);
    }
    if *sequence
        != registry
            .watermark
            .checked_add(1)
            .ok_or(ErrorCode::Capacity)?
    {
        return Err(ErrorCode::UnknownAttempt);
    }
    if !available {
        return Err(ErrorCode::Unavailable);
    }
    if registry.records.len() >= ATTEMPTS_MAX {
        return Err(ErrorCode::Capacity);
    }
    if policy != &config.policy {
        return Err(ErrorCode::IncompatiblePolicy);
    }
    if bundle != &config.bundle {
        return Err(ErrorCode::IncompatibleBundle);
    }
    // The closed workload and approved profile set. A source kind, profile,
    // workload, bundle or policy outside the fixed authority has no admitted
    // plan.
    if !crate::protocol::is_photo_workload(workload) {
        return Err(ErrorCode::InvalidRequest);
    }
    // The closed workload/source-kind pairing, plus the one recipe rule it
    // implies: a `proxy-film` source's staged bytes already carry the
    // semantic exposure transform, so any nonzero exposure would
    // double-apply it. The two RAW workloads admit only a RAW source.
    if !photo::source_kind_admitted(workload, &source.kind)
        || (workload == protocol::PHOTO_WORKLOAD_PROXY_FILM && recipe.exposure_milli_ev != 0)
    {
        return Err(ErrorCode::InvalidRequest);
    }
    if !photo_profile::APPROVED_PROFILES
        .iter()
        .any(|profile| profile.profile_id == source.profile_id)
    {
        return Err(ErrorCode::InvalidRequest);
    }
    if !EXPOSURE_MILLI_EV_RANGE.contains(&recipe.exposure_milli_ev) {
        return Err(ErrorCode::InvalidRequest);
    }
    if manifest_digest(request)? != *manifest_sha256 {
        return Err(ErrorCode::InvalidRequest);
    }
    if recipe_digest(recipe)? != *request_recipe_digest {
        return Err(ErrorCode::InvalidRequest);
    }
    if source.size > config.source_bytes_max {
        return Err(ErrorCode::InvalidRequest);
    }
    let Some(descriptor) = descriptor else {
        return Err(ErrorCode::InvalidRequest);
    };
    // Bounded metadata only; the copy happens after the intent is durable.
    photo::validate_descriptor(
        descriptor.as_raw_fd(),
        photo::DescriptorRequirement {
            kind: photo::DescriptorKind::Source,
            peer_uid: config.peer_uid,
            declared_size: source.size,
            max_bytes: config.source_bytes_max,
        },
    )
    .map_err(|_| ErrorCode::InvalidRequest)?;
    if registry
        .reserved_bytes()
        .checked_add(source.size)
        .is_none_or(|total| total > config.staged_storage_bytes_max)
    {
        return Err(ErrorCode::ResourceBudget);
    }
    if (u64::try_from(registry.unsettled()).map_err(|_| ErrorCode::Capacity)? + 1)
        * INODES_PER_ATTEMPT
        > config.staged_storage_inodes_max
    {
        return Err(ErrorCode::ResourceBudget);
    }
    // The configured control reserve and shared-ancestor headroom must be
    // measured and met before the start intent is persisted or copied.
    headroom.satisfied(config)?;
    if registry.active.is_some() {
        return Err(ErrorCode::Busy);
    }
    let time = now()?;
    let launch_id = random_id()?;
    let record = PhotoRecord {
        sequence: *sequence,
        incarnation: incarnation.clone(),
        export_id: export_id.clone(),
        policy: policy.clone(),
        bundle: bundle.clone(),
        workload: workload.clone(),
        state: State::Accepted,
        outcome: None,
        manifest_sha256: manifest_sha256.clone(),
        recipe_digest: request_recipe_digest.clone(),
        source: source.clone(),
        recipe: recipe.clone(),
        plan: None,
        phase: Phase::Intent,
        launch_id,
        image_id: Some(image_id.to_owned()),
        unit_invocation: None,
        cgroup_inode: None,
        mount_id: None,
        container_id: None,
        released: false,
        cancellation_requested: false,
        accepted_at_unix_ms: time,
        deadline_unix_ms: time
            .checked_add(ATTEMPT_DEADLINE_MS)
            .ok_or(ErrorCode::Capacity)?,
        manager_pending: None,
        stop_confirmed: false,
        evidence: None,
        output: None,
        output_transferred: false,
        validation_ack: None,
        cleanup: Cleanup::Pending,
        settled_at_unix_ms: None,
    };
    registry.active = Some(*sequence);
    registry.watermark = *sequence;
    registry.records.insert(*sequence, record.clone());
    persist(root, registry)?;
    Ok(StartAdmission::Intent(Box::new(record)))
}

/// Copy the validated descriptor into the launcher-owned private snapshot,
/// hash it against the declared identity, and seal it read-only.
pub(super) fn seal_source(
    config: &Config,
    root: &Path,
    record: &PhotoRecord,
    descriptor: &mut File,
) -> Result<photo::CopiedDescriptor, ErrorCode> {
    let workspace = record.workspace(root);
    // The attempt workspace is mode 0700, as is the launcher-owned directory
    // that holds the attempts. Both are created once and then derived again by
    // provisioning, so both creations tolerate the other's.
    let attempts = workspace.parent().ok_or(ErrorCode::Uncertain)?;
    backend::create_private_directory(attempts)?;
    backend::create_private_directory(&workspace)?;
    let source_dir = workspace.join("source");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&source_dir)
        .map_err(|_| ErrorCode::Unavailable)?;
    // The pinned engine selects its decoder from the container extension, so
    // the snapshot carries the profile's qualified container class. The
    // original filename is not part of the admitted request.
    let profile = photo_profile::APPROVED_PROFILES
        .iter()
        .find(|profile| profile.profile_id == record.source.profile_id)
        .ok_or(ErrorCode::InvalidRequest)?;
    let destination_path = source_dir.join(format!("source.{}", profile.container));
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&destination_path)
        .map_err(|_| ErrorCode::Unavailable)?;
    let copied = photo::copy_source(
        descriptor,
        &mut destination,
        config.peer_uid,
        record.source.size,
        config.source_bytes_max,
    )
    .map_err(map_seal_error)?;
    // Sync the private snapshot and seal it read-only before continuing.
    destination.sync_all().map_err(|_| ErrorCode::Unavailable)?;
    fs::set_permissions(&destination_path, fs::Permissions::from_mode(0o444))
        .map_err(|_| ErrorCode::Unavailable)?;
    // The worker runs as an unprivileged uid inside the container and must
    // traverse the staged directory to reach the sealed snapshot, so the
    // directory becomes world-traversable while the snapshot itself stays
    // sealed and nobody but the launcher can write it. Privacy comes from the
    // 0700 attempt workspace holding both. The mode is set explicitly because
    // the process umask would otherwise strip the traversal bits.
    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o755))
        .map_err(|_| ErrorCode::Unavailable)?;
    File::open(&source_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ErrorCode::Unavailable)?;
    Ok(copied)
}

fn map_seal_error(error: io::Error) -> ErrorCode {
    if matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
    ) {
        ErrorCode::InvalidRequest
    } else {
        ErrorCode::Unavailable
    }
}

/// Finalize the durable intent from the verified copy: derive the one
/// admitted stage plan, or settle the owned intent without releasing a worker.
pub(super) fn finalize_start(
    registry: &mut Registry,
    root: &Path,
    request: &Request,
    sealed: Result<photo::CopiedDescriptor, ErrorCode>,
) -> Result<ResultBody, ErrorCode> {
    let Request::Start {
        incarnation,
        export_id,
        manifest_sha256,
        sequence,
        ..
    } = request
    else {
        return Err(ErrorCode::InvalidRequest);
    };
    let record = {
        let record = registry
            .records
            .get_mut(sequence)
            .ok_or(ErrorCode::Uncertain)?;
        if record.phase != Phase::Intent
            || record.manifest_sha256 != *manifest_sha256
            || record.export_id != *export_id
            || record.incarnation != *incarnation
        {
            return Err(ErrorCode::Uncertain);
        }
        record.clone()
    };
    let verified = matches!(&sealed, Ok(copied)
        if copied.sha256 == record.source.sha256 && copied.size == record.source.size);
    let refusal = if verified {
        None
    } else {
        match &sealed {
            Ok(_) | Err(ErrorCode::InvalidRequest) => Some("refused-source-mismatch"),
            Err(_) => Some("interrupted"),
        }
    };
    if refusal.is_none() && !record.cancellation_requested {
        let record = {
            let record = registry
                .records
                .get_mut(sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.plan =
                Some(Plan::for_workload(&record.workload).ok_or(ErrorCode::InvalidRequest)?);
            record.phase = Phase::Planned;
            record.clone()
        };
        persist(root, registry)?;
        return Ok(record.result_body(incarnation));
    }
    // A mismatched copy or a concurrent cancellation settles the owned
    // intent without releasing a worker, so no second transfer can happen.
    let outcome = refusal.unwrap_or("cancelled");
    let workspace = record.workspace(root);
    let _ = fs::remove_dir_all(workspace);
    let body = {
        let record = registry
            .records
            .get_mut(sequence)
            .ok_or(ErrorCode::Uncertain)?;
        record.outcome = Some(outcome.into());
        record.state = State::Settled;
        record.settled_at_unix_ms = Some(now()?);
        record.cleanup = Cleanup::Complete;
        record.result_body(incarnation)
    };
    registry.active = None;
    persist(root, registry)?;
    match outcome {
        "interrupted" => Err(ErrorCode::Unavailable),
        "cancelled" => Ok(body),
        _ => Err(ErrorCode::InvalidRequest),
    }
}