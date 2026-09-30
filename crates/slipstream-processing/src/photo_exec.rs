//! Production Photo attempt ownership journal, provisioning and execution.
//!
//! This module owns the launcher side of the frozen production Photo protocol
//! (`design/processing-photo-protocol.md`): descriptor staging into a private
//! immutable snapshot, the isolated pinned engine attempts of the closed
//! `development-tiff`, `film-jpeg` and `proxy-film` workloads, output
//! transfer, validation acknowledgement, and durable reconciliation. It
//! never resolves Photos, reads the Library, or publishes Exports. The
//! host boundary enforcement lives in `photo_host`, the bounded Start
//! admission in `photo_admission`, and the durable registry in
//! `photo_registry`.

use crate::{
    backend, instance_claim,
    journal::{self, ManagerPhase, ParentIdentity},
    photo::{self, Config, OutputReceipt, PhotoReceipt, Recipe, Request, ResultBody, Source},
    photo_profile,
    protocol::{
        self, Availability, Cleanup, ErrorCode, Evidence, Limits, Outcome, State, digest, hex, now,
    },
    slice,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
#[path = "photo_admission.rs"]
mod photo_admission;
#[path = "photo_host.rs"]
mod photo_host;
#[path = "photo_registry.rs"]
mod photo_registry;
use photo_admission::{StartAdmission, begin_start, finalize_start, seal_source};
#[cfg(test)]
use photo_host::stop_container;
use photo_host::{Headroom, inspect_image};
#[cfg(test)]
use photo_registry::load;
use photo_registry::{
    Registry, expire, persist, prepare_private_directory, random_id, record_ref, record_ref_mut,
    restore_registry, validate_registry,
};

/// Fixed worker entrypoint of the pinned production Photo image.
pub(crate) const WORKER: &str = "/usr/local/bin/slipstream-processing-photo-worker";
const CGROUP: &str = "/sys/fs/cgroup";
/// One bounded engine window. Output transfer and validation share the
/// deadline, so ownership can never remain uncertain without a terminal path.
const ATTEMPT_DEADLINE_MS: u64 = 900_000;
const ATTEMPTS_MAX: usize = 256;
const REGISTRY_BYTES: usize = 4 * 1024 * 1024;
/// Sealed source, source dir, control dir, gate, tmpfs dir, result file,
/// output dir, and the output file: the fixed inode shape of one workspace.
const INODES_PER_ATTEMPT: u64 = 8;
/// The finite exposure range covered by the executed bundle evidence, in
/// thousandths of an EV. Values outside this range are refused at admission.
const EXPOSURE_MILLI_EV_RANGE: std::ops::RangeInclusive<i64> = 0..=1000;
const RESULT_NAME: &str = "result";

/// The launcher-owned result path of one closed workload. The
/// `development-tiff` artifact is the darktable handoff TIFF itself; the
/// `film-jpeg` artifact is the fixed Film JPEG rendered from it, and
/// `proxy-film` publishes the identical fixed sRGB Film JPEG rendered from
/// its staged Development Proxy frame. An unknown workload has no
/// published artifact.
fn output_name(workload: &str) -> Option<&'static str> {
    match workload {
        protocol::PHOTO_WORKLOAD => Some("output/development.tif"),
        protocol::PHOTO_WORKLOAD_FILM | protocol::PHOTO_WORKLOAD_PROXY_FILM => {
            Some("output/finished.jpg")
        }
        _ => None,
    }
}

// Keep this workload-to-format mapping aligned with `output_name`: the fixed
// workspace path determines both what is validated and how it is reconciled.
fn validate_output(
    workload: &str,
    path: &Path,
    max_bytes: u64,
) -> Result<OutputIdentity, ErrorCode> {
    match workload {
        protocol::PHOTO_WORKLOAD => {
            crate::photo_tiff::validate(path, max_bytes).map(|value| OutputIdentity {
                size: value.size,
                sha256: value.sha256,
                width: value.width,
                height: value.height,
            })
        }
        protocol::PHOTO_WORKLOAD_FILM | protocol::PHOTO_WORKLOAD_PROXY_FILM => {
            crate::photo_jpeg::validate(path, max_bytes).map(|value| OutputIdentity {
                size: value.size,
                sha256: value.sha256,
                width: value.width,
                height: value.height,
            })
        }
        _ => Err(ErrorCode::Uncertain),
    }
}

/// The one admitted stage plan for a closed Photo workload. The plan is
/// derived from the validated source facts, the configured bundle and the
/// finite policy; it is never a request field.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub workload: String,
    pub steps: Vec<String>,
    pub output: String,
}

impl Plan {
    /// The plan is a pure function of the closed workload: `film-jpeg`
    /// renders the fixed Film stage over the developed handoff TIFF, and
    /// `proxy-film` renders the same stage with the develop step replaced
    /// by the validated Development Proxy source. An unknown workload
    /// admits no plan.
    fn for_workload(workload: &str) -> Option<Self> {
        match workload {
            protocol::PHOTO_WORKLOAD => Some(Self {
                workload: workload.into(),
                steps: vec!["develop".into()],
                output: protocol::PHOTO_WORKLOAD.into(),
            }),
            protocol::PHOTO_WORKLOAD_FILM => Some(Self {
                workload: workload.into(),
                steps: vec!["develop".into(), "film".into()],
                output: protocol::PHOTO_WORKLOAD_FILM.into(),
            }),
            protocol::PHOTO_WORKLOAD_PROXY_FILM => Some(Self {
                workload: workload.into(),
                steps: vec!["film".into()],
                output: protocol::PHOTO_WORKLOAD_PROXY_FILM.into(),
            }),
            _ => None,
        }
    }
}

/// Validated identity of the launcher-owned Development TIFF result.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputIdentity {
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Phase {
    /// Executor intent persisted; the descriptor copy has not finished.
    Intent,
    /// Source sealed and verified; the admitted plan is persisted.
    Planned,
    /// Attempt boundary verified; the worker is about to be released.
    Provisioned,
    /// The worker was released and the engine result is being awaited.
    Released,
    /// Output validated; the attempt waits for transfer and acknowledgement.
    OutputReady,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoRecord {
    pub sequence: u64,
    pub incarnation: String,
    pub export_id: String,
    pub policy: String,
    pub bundle: String,
    /// The closed workload this attempt admits. The receipt, plan, engine
    /// grant, container command and published artifact all derive from it.
    pub workload: String,
    pub state: State,
    pub outcome: Option<String>,
    pub manifest_sha256: String,
    pub recipe_digest: String,
    pub source: Source,
    pub recipe: Recipe,
    pub plan: Option<Plan>,
    pub phase: Phase,
    pub launch_id: String,
    pub image_id: Option<String>,
    pub unit_invocation: Option<String>,
    pub cgroup_inode: Option<u64>,
    pub mount_id: Option<u64>,
    pub container_id: Option<String>,
    pub released: bool,
    pub cancellation_requested: bool,
    pub accepted_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub manager_pending: Option<ManagerPhase>,
    pub stop_confirmed: bool,
    pub evidence: Option<Evidence>,
    pub output: Option<OutputIdentity>,
    pub output_transferred: bool,
    pub validation_ack: Option<bool>,
    pub cleanup: Cleanup,
    pub settled_at_unix_ms: Option<u64>,
}

impl PhotoRecord {
    /// The bounded wire receipt for this attempt identity.
    fn wire_receipt(&self, incarnation: &str) -> PhotoReceipt {
        PhotoReceipt {
            export_id: self.export_id.clone(),
            incarnation: incarnation.to_owned(),
            sequence: self.sequence,
            workload: self.workload.clone(),
            policy: self.policy.clone(),
            bundle: self.bundle.clone(),
            state: state_name(self.state).into(),
            outcome: self.outcome.clone(),
        }
    }

    fn result_body(&self, incarnation: &str) -> ResultBody {
        ResultBody::Receipt {
            receipt: self.wire_receipt(incarnation),
        }
    }

    fn terminal(&self) -> bool {
        self.state == State::Settled || self.outcome.is_some()
    }

    fn workspace(&self, root: &Path) -> PathBuf {
        root.join("attempts").join(&self.launch_id)
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::AllocationFailed => "allocation-failed",
        Outcome::Oom => "oom",
        Outcome::StorageFull => "storage-full",
        Outcome::Cancelled => "cancelled",
        Outcome::Deadline => "deadline",
        Outcome::EngineFailed => "engine-failed",
        Outcome::Interrupted => "interrupted",
        Outcome::Unknown => "unknown",
    }
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Accepted => "accepted",
        State::Running => "running",
        State::Settling => "settling",
        State::Settled => "settled",
        State::Blocked => "blocked",
    }
}

/// The cancellation or absolute-deadline reason currently requested for
/// this attempt, if either applies.
fn requested_outcome(record: &PhotoRecord) -> Result<Option<Outcome>, ErrorCode> {
    if record.cancellation_requested {
        Ok(Some(Outcome::Cancelled))
    } else if now()? >= record.deadline_unix_ms {
        Ok(Some(Outcome::Deadline))
    } else {
        Ok(None)
    }
}

/// The canonical recipe digest: the ordered semantic tuple
/// `{exposure_milli_ev, white_balance.mode}` over compact canonical JSON.
pub fn recipe_digest(recipe: &Recipe) -> Result<String, ErrorCode> {
    let bytes = serde_json::to_vec(&serde_json::json!([
        recipe.exposure_milli_ev,
        recipe.white_balance_mode
    ]))
    .map_err(|_| ErrorCode::InvalidRequest)?;
    Ok(digest(&bytes))
}

/// The canonical manifest digest of one Start request: every field that
/// affects execution, over compact canonical JSON with sorted keys.
pub fn manifest_digest(request: &Request) -> Result<String, ErrorCode> {
    match request {
        Request::Start {
            policy,
            bundle,
            source,
            recipe,
            workload,
            ..
        } => manifest_digest_parts(source, recipe, policy, bundle, workload),
        _ => Err(ErrorCode::InvalidRequest),
    }
}

fn manifest_digest_parts(
    source: &Source,
    recipe: &Recipe,
    policy: &str,
    bundle: &str,
    workload: &str,
) -> Result<String, ErrorCode> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "bundle": bundle,
        "policy": policy,
        "recipe": [recipe.exposure_milli_ev, recipe.white_balance_mode],
        "source": {
            "kind": source.kind,
            "profile_id": source.profile_id,
            "sha256": source.sha256,
            "size": source.size,
        },
        "target": workload,
        "workload": workload,
    }))
    .map_err(|_| ErrorCode::InvalidRequest)?;
    Ok(digest(&bytes))
}

fn attempt_unit(instance: &str, launch_id: &str) -> String {
    format!("slipstreamprocessing{instance}-{launch_id}.slice")
}

fn parent_unit(instance: &str) -> String {
    format!("slipstreamprocessing{instance}.slice")
}

struct Data {
    registry: Registry,
    available: bool,
}

/// One exclusive operational owner of the production Photo capability.
pub struct PhotoExecutor {
    config: Config,
    data: Mutex<Data>,
    image_id: String,
    _lock: File,
    _instance_claim: File,
}

struct Live {
    running: bool,
    paused: bool,
    pid: u32,
    exit_code: Option<u8>,
    oom: bool,
}

impl PhotoExecutor {
    pub fn open(config: Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            return Err(ErrorCode::Unauthorized);
        }
        let root = Path::new(&config.root);
        backend::secure_directory(root, 0)?;
        backend::secure_directory(
            Path::new(&config.socket)
                .parent()
                .ok_or(ErrorCode::Unavailable)?,
            0,
        )?;
        // The instance claim is shared with the other processing executors; it
        // binds one root per instance identity across the whole host. An
        // existing claim whose root has no registry stays quarantined by the
        // shared claim path, so the only registry-less state this process can
        // observe is one it created itself.
        let authority = protocol::Config {
            version: 1,
            mode: "qualification".into(),
            instance: config.instance.clone(),
            root: config.root.clone(),
            socket: config.socket.clone(),
            peer_uid: config.peer_uid,
            image: format!("sha256:{}", "0".repeat(64)),
            memory_bytes: 128 * 1024 * 1024,
            receipt_retention_seconds: config.receipt_retention_seconds,
        };
        // The claim comes back as a lease: a claim this process created is
        // removed by the guard if any step below fails, so a refused start
        // never leaves a registry-less claim behind. The acquisition itself
        // arms the lease, so no call-site ordering can skip it; the retained
        // descriptor keeps the exclusive flock while the file is unlinked.
        let claim = instance_claim::claim_instance(&authority)?;
        // The registry is made durable before any check that can refuse the
        // start. Every later failure leaves a claim with a durable registry,
        // which the next start loads instead of re-initializing.
        let registry = restore_registry(root, &config)?;
        validate_registry(&registry, &config)?;
        persist(root, &registry)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(root.join("owner.lock"))
            .map_err(|_| ErrorCode::Unavailable)?;
        let metadata = lock.metadata().map_err(|_| ErrorCode::Unavailable)?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            return Err(ErrorCode::Unavailable);
        }
        // SAFETY: lock owns an open descriptor; flock applies to this owner lifetime.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(ErrorCode::Busy);
        }
        for name in ["attempts", "docker-client"] {
            prepare_private_directory(&root.join(name))?;
        }
        // The pinned worker image is a deployment prerequisite: an unavailable
        // or foreign image leaves the whole capability unavailable.
        let image_id = inspect_image(&config)?;
        for entry in fs::read_dir(root.join("attempts")).map_err(|_| ErrorCode::Unavailable)? {
            let entry = entry.map_err(|_| ErrorCode::Unavailable)?;
            if !registry
                .records
                .values()
                .any(|record| entry.file_name() == record.launch_id.as_str())
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        let _instance_claim = claim.take();
        Ok(Arc::new(Self {
            config,
            data: Mutex::new(Data {
                registry,
                available: false,
            }),
            image_id,
            _lock: lock,
            _instance_claim,
        }))
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Recover after a launcher restart. Ownership uncertainty keeps the
    /// capability unavailable for new work until every attempt settled.
    pub fn recover_async(self: &Arc<Self>) {
        let owner = Arc::clone(self);
        thread::Builder::new()
            .name("photo-recover".into())
            .spawn(move || {
                if owner.recover().is_err() {
                    owner.block_available();
                }
            })
            .map_err(|_| ErrorCode::Uncertain)
            .ok();
    }

    pub fn handle(
        self: &Arc<Self>,
        request: Request,
        descriptor: Option<File>,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        if request_instance(&request) != self.config.instance {
            return Err(ErrorCode::WrongInstance);
        }
        self.check_caller(peer_pid)?;
        match request {
            Request::Reconcile { .. } => self.reconcile(),
            Request::Start { .. } => self.start(request, descriptor),
            Request::Output {
                incarnation,
                sequence,
                export_id,
                ..
            } => self.output(&incarnation, &export_id, sequence, descriptor),
            Request::ValidateOutput {
                incarnation,
                sequence,
                export_id,
                size,
                sha256,
                accepted,
                ..
            } => self.validate_output(&incarnation, &export_id, sequence, size, &sha256, accepted),
            Request::Inspect {
                incarnation,
                sequence,
                export_id,
                ..
            } => self.inspect(&incarnation, &export_id, sequence),
            Request::Cancel {
                incarnation,
                sequence,
                export_id,
                ..
            } => self.cancel(&incarnation, &export_id, sequence),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Data>, ErrorCode> {
        self.data.lock().map_err(|_| ErrorCode::Uncertain)
    }

    fn capability(&self, ready: bool) -> Result<ResultBody, ErrorCode> {
        let data = self.lock()?;
        Ok(capability_body(&data.registry, &self.config, ready))
    }

    fn reconcile(&self) -> Result<ResultBody, ErrorCode> {
        let janitor = {
            let mut data = self.lock()?;
            expire(
                &mut data.registry,
                now()?,
                self.config.receipt_retention_seconds,
            )?;
            data.registry
                .records
                .iter()
                .filter(|(_, record)| {
                    record.outcome.is_some()
                        && record.state == State::Settling
                        && record.cleanup != Cleanup::Complete
                })
                .map(|(sequence, _)| *sequence)
                .collect::<Vec<_>>()
        };
        for sequence in janitor {
            // Reconcile the recorded attempt from the host before finishing a
            // settlement an earlier launcher interrupted: an attempt whose
            // manager phase the observed slice, mount and container identities
            // settle here can complete its cleanup instead of blocking
            // admission until an operator intervenes.
            let mut record = match self.record(sequence) {
                Ok(record) => record,
                Err(_) => continue,
            };
            if self.discover(&mut record).is_err() || self.update(&record).is_err() {
                continue;
            }
            let _ = self.settle_tail(sequence);
        }
        let (available, identity) = {
            let mut data = self.lock()?;
            expire(
                &mut data.registry,
                now()?,
                self.config.receipt_retention_seconds,
            )?;
            (data.available, data.registry.parent_identity.clone())
        };
        let ready = available && self.admission_ready(identity.as_ref()).is_ok();
        self.capability(ready)
    }

    fn start(
        self: &Arc<Self>,
        request: Request,
        descriptor: Option<File>,
    ) -> Result<ResultBody, ErrorCode> {
        let root = self.config.root.clone();
        // Phase A: every bounded check plus the durable executor intent.
        let admission = {
            let mut data = self.lock()?;
            let available = data.available;
            let headroom = Headroom::measure(&self.config)?;
            begin_start(
                &mut data.registry,
                &self.config,
                Path::new(&root),
                &request,
                descriptor.as_ref(),
                &self.image_id,
                available,
                &headroom,
            )?
        };
        let record = match admission {
            StartAdmission::Replay(body) => return Ok(*body),
            StartAdmission::Intent(record) => record,
        };
        // Phase B: the descriptor copy runs without the journal mutex, so
        // Inspect and Cancel stay responsive during a large staged source.
        let sealed = match descriptor {
            Some(mut file) => seal_source(&self.config, Path::new(&root), &record, &mut file),
            None => Err(ErrorCode::InvalidRequest),
        };
        // Phase C: finalize the durable intent from the verified copy.
        let body = {
            let mut data = self.lock()?;
            finalize_start(&mut data.registry, Path::new(&root), &request, sealed)?
        };
        let planned = self
            .lock()?
            .registry
            .records
            .get(&record.sequence)
            .is_some_and(|record| record.phase == Phase::Planned);
        if planned {
            let owner = Arc::clone(self);
            let sequence = record.sequence;
            thread::Builder::new()
                .name("photo-attempt".into())
                .spawn(move || {
                    if owner.execute(sequence).is_err() {
                        owner.block_available();
                    }
                })
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        Ok(body)
    }

    /// Expire settled receipts, then bind the attempt identity to the
    /// durable export identity: a foreign export never receives, mutes, or
    /// cancels an artifact.
    fn bound_record<'a>(
        &self,
        data: &'a mut Data,
        incarnation: &str,
        sequence: u64,
        export_id: &str,
    ) -> Result<&'a PhotoRecord, ErrorCode> {
        expire(
            &mut data.registry,
            now()?,
            self.config.receipt_retention_seconds,
        )?;
        let record = record_ref(&data.registry, incarnation, sequence)?;
        if record.export_id != export_id {
            return Err(ErrorCode::Conflict);
        }
        Ok(record)
    }

    /// Apply one bounded edit to the durable record under the journal lock.
    fn edit_record(
        &self,
        sequence: u64,
        edit: impl FnOnce(&mut PhotoRecord),
    ) -> Result<(), ErrorCode> {
        let mut data = self.lock()?;
        let record = data
            .registry
            .records
            .get_mut(&sequence)
            .ok_or(ErrorCode::Uncertain)?;
        edit(record);
        persist(Path::new(&self.config.root), &data.registry)
    }

    fn output(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
        mut descriptor: Option<File>,
    ) -> Result<ResultBody, ErrorCode> {
        let (identity, result_path, mut descriptor, workload) = {
            let mut data = self.lock()?;
            let record = self.bound_record(&mut data, incarnation, sequence, export_id)?;
            // The durable claim admits exactly one service artifact; once it
            // is spent, no retry or concurrent request may transfer again.
            if record.output_transferred {
                return Err(ErrorCode::Conflict);
            }
            if record.phase != Phase::OutputReady {
                return Err(ErrorCode::InvalidRequest);
            }
            // The descriptor is verified before anything durable changes, so
            // an unusable descriptor never spends the attempt's one claim.
            let descriptor = descriptor.take().ok_or(ErrorCode::InvalidRequest)?;
            photo::validate_descriptor(
                descriptor.as_raw_fd(),
                photo::DescriptorRequirement {
                    kind: photo::DescriptorKind::Output,
                    peer_uid: self.config.peer_uid,
                    declared_size: 0,
                    max_bytes: self.config.output_bytes_max,
                },
            )
            .map_err(|_| ErrorCode::InvalidRequest)?;
            let identity = record.output.clone().ok_or(ErrorCode::Uncertain)?;
            let name = output_name(&record.workload).ok_or(ErrorCode::Uncertain)?;
            let path = record
                .workspace(Path::new(&self.config.root))
                .join("work")
                .join(name);
            let workload = record.workload.clone();
            // Persist the transfer claim under the journal lock before the
            // copy: concurrent requests observe the claim and are refused,
            // and one attempt can never publish a second service artifact.
            // A failed copy keeps the claim, so the attempt settles without
            // a second transfer instead of publishing partial bytes twice.
            record_ref_mut(&mut data.registry, incarnation, sequence)?.output_transferred = true;
            persist(Path::new(&self.config.root), &data.registry)?;
            (identity, path, descriptor, workload)
        };
        let receipt = transfer_output(
            &self.config,
            &identity,
            &result_path,
            &mut descriptor,
            export_id,
            incarnation,
            sequence,
            &workload,
        )?;
        Ok(ResultBody::Output { receipt })
    }

    fn validate_output(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
        size: u64,
        sha256: &str,
        accepted: bool,
    ) -> Result<ResultBody, ErrorCode> {
        let mut data = self.lock()?;
        let record = self.bound_record(&mut data, incarnation, sequence, export_id)?;
        if record.terminal() {
            return Ok(record.result_body(incarnation));
        }
        if record.validation_ack == Some(accepted) {
            return Ok(record.result_body(incarnation));
        }
        if record.validation_ack.is_some() {
            return Err(ErrorCode::Conflict);
        }
        if !record.output_transferred {
            return Err(ErrorCode::InvalidRequest);
        }
        let output = record.output.as_ref().ok_or(ErrorCode::Uncertain)?;
        if output.size != size || output.sha256 != sha256 {
            return Err(ErrorCode::Conflict);
        }
        // The acknowledgement is not part of the wire receipt, so the bound
        // record already carries the body the edit settles to.
        let body = record.result_body(incarnation);
        record_ref_mut(&mut data.registry, incarnation, sequence)?.validation_ack = Some(accepted);
        persist(Path::new(&self.config.root), &data.registry)?;
        Ok(body)
    }

    fn inspect(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
    ) -> Result<ResultBody, ErrorCode> {
        let mut data = self.lock()?;
        let record = self.bound_record(&mut data, incarnation, sequence, export_id)?;
        Ok(record.result_body(incarnation))
    }

    fn cancel(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
    ) -> Result<ResultBody, ErrorCode> {
        let mut data = self.lock()?;
        let record = self.bound_record(&mut data, incarnation, sequence, export_id)?;
        if record.terminal() {
            return Ok(record.result_body(incarnation));
        }
        // The cancellation flag is not part of the wire receipt, so the
        // bound record already carries the body the edit settles to.
        let body = record.result_body(incarnation);
        record_ref_mut(&mut data.registry, incarnation, sequence)?.cancellation_requested = true;
        persist(Path::new(&self.config.root), &data.registry)?;
        Ok(body)
    }

    fn update(&self, record: &PhotoRecord) -> Result<(), ErrorCode> {
        let mut data = self.lock()?;
        let sequence = record.sequence;
        let previous = data
            .registry
            .records
            .get(&sequence)
            .ok_or(ErrorCode::Uncertain)?
            .clone();
        let mut record = record.clone();
        // A concurrent cancellation and the first terminal reason are never lost.
        record.cancellation_requested |= previous.cancellation_requested;
        record.outcome = previous.outcome.or(record.outcome);
        record.stop_confirmed |= previous.stop_confirmed;
        data.registry.records.insert(sequence, record);
        persist(Path::new(&self.config.root), &data.registry)
    }

    fn record(&self, sequence: u64) -> Result<PhotoRecord, ErrorCode> {
        self.lock()?
            .registry
            .records
            .get(&sequence)
            .cloned()
            .ok_or(ErrorCode::Uncertain)
    }

    fn block_available(&self) {
        if let Ok(mut data) = self.lock() {
            data.available = false;
            let _ = persist(Path::new(&self.config.root), &data.registry);
        }
    }

    // Execution -----------------------------------------------------------

    fn execute(self: &Arc<Self>, sequence: u64) -> Result<(), ErrorCode> {
        let identity = self.lock()?.registry.parent_identity.clone();
        self.verify_parent(identity.as_ref())?;
        let mut record = self.record(sequence)?;
        let mut gate = match self.provision(&mut record) {
            Ok(gate) => gate,
            Err(_) => {
                let _ = self.discover(&mut record);
                let _ = self.update(&record);
                self.settle(sequence, Outcome::Interrupted)?;
                return Ok(());
            }
        };
        // Holding the journal mutex closes the cancel/release race.
        {
            let data = self.lock()?;
            let current = data
                .registry
                .records
                .get(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            if let Some(reason) = requested_outcome(current)? {
                drop(data);
                return self.settle(sequence, reason).map(|_| ());
            }
            gate.write_all(format!("{}\n", record.launch_id).as_bytes())
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        self.release(&mut record)?;
        // The worker reads the release token until end of file, so the write
        // end must close before the worker can pass its gate. The reference
        // release does the same, and holding it open would deadlock the worker
        // until its absolute deadline.
        drop(gate);
        record.phase = Phase::Released;
        record.state = State::Running;
        self.update(&record)?;
        loop {
            let current = self.record(sequence)?;
            if !self.live(&current)?.running {
                break;
            }
            if let Some(reason) = requested_outcome(&current)? {
                return self.settle(sequence, reason).map(|_| ());
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.finish(sequence)
    }

    fn finish(self: &Arc<Self>, sequence: u64) -> Result<(), ErrorCode> {
        let mut record = self.record(sequence)?;
        self.stop_and_wait(&mut record)?;
        let evidence = self.terminal_evidence(&record)?;
        let worker = self.worker_outcome(&record)?;
        let requested = requested_outcome(&record)?;
        let outcome = journal::classify(&evidence, worker, requested);
        // A completed attempt publishes only a validated artifact; a failed
        // validation settles with the output-validation refusal instead.
        let output = if outcome == Outcome::Completed {
            let Some(name) = output_name(&record.workload) else {
                return Err(ErrorCode::Uncertain);
            };
            let output_path = record.workspace(Path::new(&self.config.root));
            let output_path = output_path.join("work").join(name);
            validate_output(&record.workload, &output_path, self.config.output_bytes_max).ok()
        } else {
            None
        };
        let published = output.is_some();
        let outcome_text = if !published && outcome == Outcome::Completed {
            Some("refused-output-validation")
        } else if outcome != Outcome::Completed {
            Some(outcome_name(outcome))
        } else {
            None
        };
        self.edit_record(sequence, |record| {
            if record.outcome.is_none()
                && let Some(text) = outcome_text
            {
                record.outcome = Some(text.into());
            }
            record.evidence = Some(evidence);
            if let Some(identity) = output {
                record.output = Some(identity);
                record.phase = Phase::OutputReady;
            }
            record.state = State::Settling;
        })?;
        if published {
            return self.wait_settled(sequence);
        }
        self.settle_tail(sequence)?;
        Ok(())
    }

    fn wait_settled(self: &Arc<Self>, sequence: u64) -> Result<(), ErrorCode> {
        loop {
            let record = self.record(sequence)?;
            if let Some(accepted) = record.validation_ack {
                self.edit_record(sequence, |record| {
                    if record.outcome.is_none() {
                        record.outcome = Some(if accepted {
                            "completed".into()
                        } else {
                            "refused-output-validation".into()
                        });
                    }
                    record.state = State::Settling;
                })?;
                self.settle_tail(sequence)?;
                return Ok(());
            }
            if let Some(reason) = requested_outcome(&record)? {
                return self.settle(sequence, reason).map(|_| ());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Mark the one terminal outcome. An already terminal attempt is returned
    /// unchanged, so settlement happens exactly once against actual completion.
    fn settle(&self, sequence: u64, requested: Outcome) -> Result<ResultBody, ErrorCode> {
        {
            let mut data = self.lock()?;
            let incarnation = data.registry.incarnation.clone();
            let record = data
                .registry
                .records
                .get_mut(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            if record.terminal() {
                return Ok(record.result_body(&incarnation));
            }
            record.outcome = Some(outcome_name(requested).into());
            record.state = State::Settling;
            persist(Path::new(&self.config.root), &data.registry)?;
        }
        self.settle_tail(sequence)
    }

    /// Stop a still-running worker and wait for its confirmed exit.
    fn stop_and_wait(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if self.live(record).is_ok_and(|live| live.running) {
            self.stop(record)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.live(record)?.running {
                if Instant::now() >= deadline {
                    return Err(ErrorCode::Uncertain);
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    }

    /// Terminal settlement: stop the worker, capture the terminal resource
    /// evidence, remove the attempt boundary, and settle the receipt durably.
    fn settle_tail(&self, sequence: u64) -> Result<ResultBody, ErrorCode> {
        let mut record = self.record(sequence)?;
        self.stop_and_wait(&mut record)?;
        let evidence = if record.unit_invocation.is_some() {
            Some(self.terminal_evidence(&record)?)
        } else if record.evidence.is_none() {
            // An attempt that never reached its attempt boundary still
            // settles with explicit unpopulated evidence.
            Some(Evidence {
                peak_bytes: 0,
                exit_code: None,
                docker_oom_killed: None,
                attempt_before: None,
                attempt_after: None,
                parent_before: None,
                parent_after: None,
                populated: Some(false),
                terminal_snapshot: None,
            })
        } else {
            None
        };
        if let Some(evidence) = evidence {
            self.edit_record(sequence, |record| record.evidence = Some(evidence))?;
        }
        self.cleanup(&mut record)?;
        let body = {
            let mut data = self.lock()?;
            let incarnation = data.registry.incarnation.clone();
            let record = data
                .registry
                .records
                .get_mut(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.cleanup = Cleanup::Complete;
            record.state = State::Settled;
            record.settled_at_unix_ms = Some(now()?);
            let body = record.result_body(&incarnation);
            if data.registry.active == Some(sequence) {
                data.registry.active = None;
            }
            persist(Path::new(&self.config.root), &data.registry)?;
            body
        };
        Ok(body)
    }

    fn recover(self: &Arc<Self>) -> Result<(), ErrorCode> {
        let (records, parent) = {
            let data = self.lock()?;
            if data.registry.parent_pending {
                return Err(ErrorCode::Uncertain);
            }
            (
                data.registry.records.values().cloned().collect::<Vec<_>>(),
                data.registry.parent_identity.clone(),
            )
        };
        self.verify_parent(parent.as_ref())?;
        self.scan_unowned(&records)?;
        let mut waiters = Vec::new();
        for mut record in records {
            if record.state == State::Settled {
                continue;
            }
            self.discover(&mut record)?;
            self.update(&record)?;
            if record.phase == Phase::OutputReady
                && record.validation_ack.is_none()
                && !record.terminal()
                && record
                    .output
                    .as_ref()
                    .is_some_and(|identity| self.output_present(&record, identity))
            {
                waiters.push(record.sequence);
                continue;
            }
            let requested = requested_outcome(&record)?.unwrap_or(Outcome::Interrupted);
            self.settle(record.sequence, requested)?;
        }
        {
            let mut data = self.lock()?;
            data.registry.parent_pending = true;
            persist(Path::new(&self.config.root), &data.registry)?;
        }
        let identity = self.prepare_parent(parent.as_ref())?;
        {
            let mut data = self.lock()?;
            data.registry.parent_identity = Some(identity);
            data.registry.parent_pending = false;
            persist(Path::new(&self.config.root), &data.registry)?;
            data.available = true;
        }
        for sequence in waiters {
            let owner = Arc::clone(self);
            thread::Builder::new()
                .name("photo-output-wait".into())
                .spawn(move || {
                    let _ = owner.wait_settled(sequence);
                })
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        Ok(())
    }

    fn output_present(&self, record: &PhotoRecord, identity: &OutputIdentity) -> bool {
        let Some(name) = output_name(&record.workload) else {
            return false;
        };
        let path = record
            .workspace(Path::new(&self.config.root))
            .join("work")
            .join(name);
        let found = validate_output(&record.workload, &path, self.config.output_bytes_max);
        found.is_ok_and(|found| {
            found.size == identity.size
                && found.sha256 == identity.sha256
                && found.width == identity.width
                && found.height == identity.height
        })
    }
}

/// Copy the launcher-owned result into the service output descriptor with
/// bounded chunks and return the bounded OutputReceipt. The descriptor has
/// already been validated by the caller, and the durable transfer claim has
/// already been persisted, so this is the one copy of the one artifact.
#[allow(clippy::too_many_arguments)]
fn transfer_output(
    config: &Config,
    identity: &OutputIdentity,
    result_path: &Path,
    descriptor: &mut File,
    export_id: &str,
    incarnation: &str,
    sequence: u64,
    target: &str,
) -> Result<OutputReceipt, ErrorCode> {
    let mut result = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(result_path)
        .map_err(|_| ErrorCode::Uncertain)?;
    let metadata = result.metadata().map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() != identity.size
        || metadata.len() > config.output_bytes_max
    {
        return Err(ErrorCode::Uncertain);
    }
    let mut remaining = identity.size;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let take = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| ErrorCode::Uncertain)?;
        let read = result
            .read(&mut buffer[..take])
            .map_err(|_| ErrorCode::Uncertain)?;
        if read == 0 {
            return Err(ErrorCode::Uncertain);
        }
        descriptor
            .write_all(&buffer[..read])
            .map_err(|_| ErrorCode::Uncertain)?;
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    descriptor.sync_all().map_err(|_| ErrorCode::Uncertain)?;
    let sha256 = format!("{:x}", hasher.finalize());
    if sha256 != identity.sha256 {
        return Err(ErrorCode::Uncertain);
    }
    Ok(OutputReceipt {
        export_id: export_id.to_owned(),
        incarnation: incarnation.to_owned(),
        sequence,
        target: target.to_owned(),
        size: identity.size,
        sha256,
    })
}
fn request_instance(request: &Request) -> &str {
    match request {
        Request::Reconcile { instance, .. }
        | Request::Start { instance, .. }
        | Request::Output { instance, .. }
        | Request::ValidateOutput { instance, .. }
        | Request::Inspect { instance, .. }
        | Request::Cancel { instance, .. } => instance,
    }
}

fn capability_body(registry: &Registry, config: &Config, ready: bool) -> ResultBody {
    ResultBody::Capability {
        capability: protocol::PHOTO_CAPABILITY.into(),
        instance: config.instance.clone(),
        incarnation: registry.incarnation.clone(),
        next_sequence: registry.watermark.saturating_add(1),
        policy: config.policy.clone(),
        bundle: config.bundle.clone(),
        availability: if ready {
            Availability::Available
        } else {
            Availability::Blocked
        },
        active: registry.active.and_then(|sequence| {
            registry
                .records
                .get(&sequence)
                .filter(|record| record.state != State::Settled)
                .map(|record| record.wire_receipt(&registry.incarnation))
        }),
    }
}

#[cfg(test)]
#[path = "photo_exec_tests.rs"]
mod tests;