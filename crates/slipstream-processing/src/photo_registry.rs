//! Durable production Photo registry: strict snapshots and atomic persistence.

use super::{
    ATTEMPTS_MAX, Cleanup, Config, EXPOSURE_MILLI_EV_RANGE, ErrorCode, OUTCOMES, Phase,
    PhotoRecord, Plan, REGISTRY_BYTES, State, hex, manifest_digest_parts, recipe_digest,
};
use crate::journal::ParentIdentity;
use crate::{backend, photo, photo_profile, protocol};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registry {
    pub(super) version: u8,
    pub(super) instance: String,
    pub(super) incarnation: String,
    pub(super) watermark: u64,
    pub(super) parent_pending: bool,
    pub(super) parent_identity: Option<ParentIdentity>,
    pub(super) active: Option<u64>,
    pub(super) records: BTreeMap<u64, PhotoRecord>,
}

impl Registry {
    pub(super) fn reserved_bytes(&self) -> u64 {
        self.records
            .values()
            .filter(|record| record.state != State::Settled)
            .map(|record| record.source.size)
            .sum()
    }

    pub(super) fn unsettled(&self) -> usize {
        self.records
            .values()
            .filter(|record| record.state != State::Settled)
            .count()
    }
}

// Durable snapshot -----------------------------------------------------

pub(super) fn record_ref<'a>(
    registry: &'a Registry,
    incarnation: &str,
    sequence: u64,
) -> Result<&'a PhotoRecord, ErrorCode> {
    if incarnation != registry.incarnation {
        return Err(ErrorCode::StaleIncarnation);
    }
    registry
        .records
        .get(&sequence)
        .ok_or(if sequence <= registry.watermark {
            ErrorCode::Expired
        } else {
            ErrorCode::UnknownAttempt
        })
}

pub(super) fn record_ref_mut<'a>(
    registry: &'a mut Registry,
    incarnation: &str,
    sequence: u64,
) -> Result<&'a mut PhotoRecord, ErrorCode> {
    if incarnation != registry.incarnation {
        return Err(ErrorCode::StaleIncarnation);
    }
    registry
        .records
        .get_mut(&sequence)
        .ok_or(if sequence <= registry.watermark {
            ErrorCode::Expired
        } else {
            ErrorCode::UnknownAttempt
        })
}

pub(super) fn expire(registry: &mut Registry, time: u64, retention: u64) -> Result<(), ErrorCode> {
    let retention = retention.checked_mul(1000).ok_or(ErrorCode::Capacity)?;
    registry.records.retain(|_, record| {
        record.state != State::Settled
            || record.cleanup != Cleanup::Complete
            || record
                .settled_at_unix_ms
                .and_then(|settled| time.checked_sub(settled))
                .is_none_or(|age| age < retention)
    });
    Ok(())
}

/// The durable snapshot is tamper-checked and internally coherent. A record
/// can never claim success without a validated output and completed cleanup,
/// and an unsettled record never claims a cleanup. A terminal outcome with a
/// pending cleanup is the launcher's own intermediate settlement state: the
/// outcome is persisted before the attempt boundary is removed, and
/// `reconcile` retries that cleanup on the next start, so it must load.
pub(super) fn validate_registry(registry: &Registry, config: &Config) -> Result<(), ErrorCode> {
    if registry.version != 1
        || registry.instance != config.instance
        || !hex(&registry.incarnation, 32)
        || registry.records.len() > ATTEMPTS_MAX
        || (!registry.records.is_empty() && registry.parent_identity.is_none())
        || registry
            .parent_identity
            .as_ref()
            .is_some_and(|identity| !hex(&identity.invocation, 32) || identity.inode == 0)
    {
        return Err(ErrorCode::Uncertain);
    }
    let mut active = None;
    for (sequence, record) in &registry.records {
        if *sequence == 0
            || *sequence > registry.watermark
            || record.sequence != *sequence
            || record.incarnation != registry.incarnation
            || !photo::identifier(&record.export_id, 128)
            || !hex(&record.policy, 64)
            || !hex(&record.bundle, 64)
            || record.policy != config.policy
            || record.bundle != config.bundle
            || !matches!(
                record.outcome.as_deref(),
                None | Some(
                    "completed"
                        | "allocation-failed"
                        | "oom"
                        | "storage-full"
                        | "cancelled"
                        | "deadline"
                        | "engine-failed"
                        | "interrupted"
                        | "unknown"
                        | "refused-source-mismatch"
                        | "refused-output-validation"
                )
            )
            || record
                .outcome
                .as_deref()
                .is_some_and(|outcome| !OUTCOMES.contains(&outcome))
            || (record.outcome.is_none() && record.cleanup != Cleanup::Pending)
            || (record.state == State::Settled)
                != (record.outcome.is_some() && record.cleanup == Cleanup::Complete)
            || (record.state == State::Settled
                && (record.settled_at_unix_ms.is_none()
                    || record.outcome.is_none()
                    || record.cleanup != Cleanup::Complete))
            || record.outcome.is_some()
                != (record.state == State::Settling || record.state == State::Settled)
            // The durable record carries the same closed pairing the
            // request admitted, including the zero exposure a
            // `proxy-film` Development Proxy source requires.
            || !photo::source_kind_admitted(&record.workload, &record.source.kind)
            || (record.workload == protocol::PHOTO_WORKLOAD_PROXY_FILM
                && record.recipe.exposure_milli_ev != 0)
            || !photo::identifier(&record.source.profile_id, 64)
            || !photo_profile::APPROVED_PROFILES
                .iter()
                .any(|profile| profile.profile_id == record.source.profile_id)
            || record.source.size == 0
            || record.source.size > config.source_bytes_max
            || !hex(&record.source.sha256, 64)
            || record.recipe.white_balance_mode != "as-shot"
            || !EXPOSURE_MILLI_EV_RANGE.contains(&record.recipe.exposure_milli_ev)
            || !hex(&record.manifest_sha256, 64)
            || !hex(&record.recipe_digest, 64)
            || manifest_digest_parts(
                &record.source,
                &record.recipe,
                &record.policy,
                &record.bundle,
                &record.workload,
            ) != Ok(record.manifest_sha256.clone())
            || recipe_digest(&record.recipe) != Ok(record.recipe_digest.clone())
            || record.accepted_at_unix_ms == 0
            || record.deadline_unix_ms < record.accepted_at_unix_ms
            || !hex(&record.launch_id, 32)
            || !protocol::is_photo_workload(&record.workload)
            || record
                .image_id
                .as_ref()
                .is_some_and(|image| !image.strip_prefix("sha256:").is_some_and(|id| hex(id, 64)))
            || record
                .unit_invocation
                .as_ref()
                .is_some_and(|invocation| !hex(invocation, 32))
            || record.container_id.as_ref().is_some_and(|id| !hex(id, 64))
            || (record.released && record.container_id.is_none())
            || (record.stop_confirmed && record.unit_invocation.is_none())
            || (record.manager_pending.is_some() && record.state == State::Settled)
        {
            return Err(ErrorCode::Uncertain);
        }
        match record.phase {
            Phase::Intent => {
                if record.plan.is_some() {
                    return Err(ErrorCode::Uncertain);
                }
            }
            Phase::Planned | Phase::Provisioned | Phase::Released | Phase::OutputReady => {
                if Plan::for_workload(&record.workload)
                    .is_none_or(|expected| record.plan.as_ref() != Some(&expected))
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        match &record.output {
            Some(output) => {
                if record.phase != Phase::OutputReady
                    || output.size == 0
                    || output.size > config.output_bytes_max
                    || !hex(&output.sha256, 64)
                    || output.width == 0
                    || output.height == 0
                    || (!record.output_transferred && record.validation_ack.is_some())
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
            None => {
                if record.phase == Phase::OutputReady
                    || record.output_transferred
                    || record.validation_ack.is_some()
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        if record.state != State::Settled && active.replace(*sequence).is_some() {
            return Err(ErrorCode::Uncertain);
        }
    }
    if active != registry.active {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

pub(super) fn prepare_private_directory(path: &Path) -> Result<(), ErrorCode> {
    if !path.try_exists().map_err(|_| ErrorCode::Unavailable)? {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| ErrorCode::Unavailable)?;
    }
    // The production launcher runs as root, so this is the root-owned check;
    // focused tests exercise the same path as the invoking user.
    backend::secure_directory(path, unsafe { libc::geteuid() })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ErrorCode::Unavailable)
}

fn owned_by_self(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.nlink() == 1
        && metadata.mode() & 0o077 == 0
}

pub(super) fn load(root: &Path) -> Result<Option<Registry>, ErrorCode> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("registry.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    // The production launcher runs as root, so this is the root-owned check;
    // focused tests exercise the same path as the invoking user.
    if !owned_by_self(&metadata) {
        return Err(ErrorCode::Uncertain);
    }
    let mut bytes = Vec::new();
    file.take(REGISTRY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > REGISTRY_BYTES {
        return Err(ErrorCode::Uncertain);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ErrorCode::Uncertain)
}

pub(super) fn persist(root: &Path, registry: &Registry) -> Result<(), ErrorCode> {
    let bytes = serde_json::to_vec(registry).map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > REGISTRY_BYTES {
        return Err(ErrorCode::Capacity);
    }
    let temporary = root.join("registry.next");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .map_err(|_| ErrorCode::Uncertain)?;
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    if !owned_by_self(&metadata) {
        return Err(ErrorCode::Uncertain);
    }
    file.set_len(0)
        .and_then(|_| file.write_all(&bytes))
        .and_then(|_| file.sync_all())
        .map_err(|_| ErrorCode::Uncertain)?;
    fs::rename(&temporary, root.join("registry.json")).map_err(|_| ErrorCode::Uncertain)?;
    File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(|_| ErrorCode::Uncertain)
}

pub(super) fn random_id() -> Result<String, ErrorCode> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| ErrorCode::Unavailable)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Restore the durable registry, or initialize a fresh one with a new
/// incarnation, exactly as a first start. An existing claim whose root has
/// no registry never reaches here: the shared claim path quarantines it.
pub(super) fn restore_registry(root: &Path, config: &Config) -> Result<Registry, ErrorCode> {
    Ok(load(root)?.unwrap_or(Registry {
        version: 1,
        instance: config.instance.clone(),
        incarnation: random_id()?,
        watermark: 0,
        parent_pending: false,
        parent_identity: None,
        active: None,
        records: BTreeMap::new(),
    }))
}
