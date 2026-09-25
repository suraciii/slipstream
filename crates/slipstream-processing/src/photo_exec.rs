//! Production Photo attempt ownership journal, provisioning and execution.
//!
//! This module owns the launcher side of the frozen production Photo protocol
//! (`design/processing-photo-protocol.md`): descriptor staging into a private
//! immutable snapshot, the isolated pinned `development-tiff` engine attempt,
//! output transfer, validation acknowledgement, and durable reconciliation.
//! It never resolves Photos, reads the Library, or publishes Exports.

use crate::{
    backend,
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
    collections::BTreeMap,
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
/// Terminal outcomes a settled receipt may carry.
const OUTCOMES: [&str; 11] = [
    "completed",
    "allocation-failed",
    "oom",
    "storage-full",
    "cancelled",
    "deadline",
    "engine-failed",
    "interrupted",
    "unknown",
    "refused-source-mismatch",
    "refused-output-validation",
];
const RESULT_NAME: &str = "result";
const OUTPUT_NAME: &str = "output/development.tif";

/// The one admitted stage plan for the closed `development-tiff` workload.
/// The plan is derived from the validated source facts, the configured bundle
/// and the finite policy; it is never a request field.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub workload: String,
    pub steps: Vec<String>,
    pub output: String,
}

impl Plan {
    fn development_tiff() -> Self {
        Self {
            workload: protocol::PHOTO_WORKLOAD.into(),
            steps: vec!["develop".into()],
            output: protocol::PHOTO_WORKLOAD.into(),
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
            workload: protocol::PHOTO_WORKLOAD.into(),
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
            ..
        } => manifest_digest_of(source, recipe, policy, bundle),
        _ => Err(ErrorCode::InvalidRequest),
    }
}

fn manifest_digest_parts(
    source: &Source,
    recipe: &Recipe,
    policy: &str,
    bundle: &str,
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
        "target": protocol::PHOTO_WORKLOAD,
        "workload": protocol::PHOTO_WORKLOAD,
    }))
    .map_err(|_| ErrorCode::InvalidRequest)?;
    Ok(digest(&bytes))
}

fn recipe_digest_of(recipe: &Recipe) -> Result<String, ErrorCode> {
    recipe_digest(recipe)
}

fn manifest_digest_of(
    source: &Source,
    recipe: &Recipe,
    policy: &str,
    bundle: &str,
) -> Result<String, ErrorCode> {
    manifest_digest_parts(source, recipe, policy, bundle)
}

fn attempt_unit(instance: &str, launch_id: &str) -> String {
    format!("slipstreamprocessing{instance}-{launch_id}.slice")
}

fn parent_unit(instance: &str) -> String {
    format!("slipstreamprocessing{instance}.slice")
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u8,
    instance: String,
    incarnation: String,
    watermark: u64,
    parent_pending: bool,
    parent_identity: Option<ParentIdentity>,
    active: Option<u64>,
    records: BTreeMap<u64, PhotoRecord>,
}

impl Registry {
    fn reserved_bytes(&self) -> u64 {
        self.records
            .values()
            .filter(|record| record.state != State::Settled)
            .map(|record| record.source.size)
            .sum()
    }

    fn unsettled(&self) -> usize {
        self.records
            .values()
            .filter(|record| record.state != State::Settled)
            .count()
    }
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
    pid: u32,
    exit_code: Option<u8>,
    oom: bool,
}

/// The outcome of the bounded admission checks of one Start request.
#[derive(Debug)]
enum StartAdmission {
    /// A durable receipt for this attempt identity already exists.
    Replay(Box<ResultBody>),
    /// A fresh executor intent was persisted for the copied descriptor.
    Intent(Box<PhotoRecord>),
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
        let claim = journal::claim_instance(&authority)?;
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

    fn limits(&self) -> Limits {
        Limits {
            memory_bytes: self.config.memory_bytes,
            swap_bytes: 0,
            cpu_quota_us: self.config.cpu_quota_us,
            cpu_period_us: 100_000,
            tasks: u64::from(self.config.tasks),
            storage_bytes: self.config.staged_storage_bytes_max,
            storage_inodes: self.config.staged_storage_inodes_max,
        }
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
            finalize_start(
                &mut data.registry,
                &self.config,
                Path::new(&root),
                &request,
                sealed,
            )?
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

    fn output(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
        descriptor: Option<File>,
    ) -> Result<ResultBody, ErrorCode> {
        let mut descriptor = descriptor;
        let (identity, result_path, mut descriptor) = {
            let mut data = self.lock()?;
            expire(
                &mut data.registry,
                now()?,
                self.config.receipt_retention_seconds,
            )?;
            let record = record_ref(&data.registry, incarnation, sequence)?;
            // The attempt identity is bound to the durable export identity:
            // a foreign export never receives, mutes, or cancels an artifact.
            if record.export_id != export_id {
                return Err(ErrorCode::Conflict);
            }
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
            let path = record
                .workspace(Path::new(&self.config.root))
                .join("work")
                .join(OUTPUT_NAME);
            // Persist the transfer claim under the journal lock before the
            // copy: concurrent requests observe the claim and are refused,
            // and one attempt can never publish a second service artifact.
            // A failed copy keeps the claim, so the attempt settles without
            // a second transfer instead of publishing partial bytes twice.
            record_ref_mut(&mut data.registry, incarnation, sequence)?.output_transferred = true;
            persist(Path::new(&self.config.root), &data.registry)?;
            (identity, path, descriptor)
        };
        let receipt = transfer_output(
            &self.config,
            &identity,
            &result_path,
            &mut descriptor,
            export_id,
            incarnation,
            sequence,
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
        expire(
            &mut data.registry,
            now()?,
            self.config.receipt_retention_seconds,
        )?;
        let record = record_ref(&data.registry, incarnation, sequence)?;
        if record.export_id != export_id {
            return Err(ErrorCode::Conflict);
        }
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
        let record = {
            let record = record_ref_mut(&mut data.registry, incarnation, sequence)?;
            record.validation_ack = Some(accepted);
            record.clone()
        };
        persist(Path::new(&self.config.root), &data.registry)?;
        Ok(record.result_body(incarnation))
    }

    fn inspect(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
    ) -> Result<ResultBody, ErrorCode> {
        let mut data = self.lock()?;
        expire(
            &mut data.registry,
            now()?,
            self.config.receipt_retention_seconds,
        )?;
        let record = record_ref(&data.registry, incarnation, sequence)?;
        if record.export_id != export_id {
            return Err(ErrorCode::Conflict);
        }
        Ok(record.result_body(incarnation))
    }

    fn cancel(
        &self,
        incarnation: &str,
        export_id: &str,
        sequence: u64,
    ) -> Result<ResultBody, ErrorCode> {
        let mut data = self.lock()?;
        expire(
            &mut data.registry,
            now()?,
            self.config.receipt_retention_seconds,
        )?;
        let record = record_ref(&data.registry, incarnation, sequence)?;
        if record.export_id != export_id {
            return Err(ErrorCode::Conflict);
        }
        if record.terminal() {
            return Ok(record.result_body(incarnation));
        }
        let record = {
            let record = record_ref_mut(&mut data.registry, incarnation, sequence)?;
            record.cancellation_requested = true;
            record.clone()
        };
        persist(Path::new(&self.config.root), &data.registry)?;
        Ok(record.result_body(incarnation))
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
            let cancelled = current.cancellation_requested;
            let expired = now()? >= current.deadline_unix_ms;
            if cancelled || expired {
                drop(data);
                return self
                    .settle(
                        sequence,
                        if cancelled {
                            Outcome::Cancelled
                        } else {
                            Outcome::Deadline
                        },
                    )
                    .map(|_| ());
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
            if current.cancellation_requested {
                return self.settle(sequence, Outcome::Cancelled).map(|_| ());
            }
            if now()? >= current.deadline_unix_ms {
                return self.settle(sequence, Outcome::Deadline).map(|_| ());
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.finish(sequence)
    }

    fn finish(self: &Arc<Self>, sequence: u64) -> Result<(), ErrorCode> {
        let record = self.record(sequence)?;
        if self.live(&record).is_ok_and(|live| live.running) {
            self.stop(&record)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.live(&record)?.running {
                if Instant::now() >= deadline {
                    return Err(ErrorCode::Uncertain);
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        let evidence = self.terminal_evidence(&record)?;
        let worker = self.worker_outcome(&record)?;
        let requested = if record.cancellation_requested {
            Some(Outcome::Cancelled)
        } else if now()? >= record.deadline_unix_ms {
            Some(Outcome::Deadline)
        } else {
            None
        };
        let outcome = journal::classify(&evidence, worker, requested);
        if outcome == Outcome::Completed {
            let output_path = record.workspace(Path::new(&self.config.root));
            let output_path = output_path.join("work").join(OUTPUT_NAME);
            match crate::photo_tiff::validate(&output_path, self.config.output_bytes_max) {
                Ok(identity) => {
                    let identity = OutputIdentity {
                        size: identity.size,
                        sha256: identity.sha256,
                        width: identity.width,
                        height: identity.height,
                    };
                    let mut data = self.lock()?;
                    let record = data
                        .registry
                        .records
                        .get_mut(&sequence)
                        .ok_or(ErrorCode::Uncertain)?;
                    record.evidence = Some(evidence);
                    record.output = Some(identity);
                    record.phase = Phase::OutputReady;
                    record.state = State::Settling;
                    persist(Path::new(&self.config.root), &data.registry)?;
                    drop(data);
                    return self.wait_settled(sequence);
                }
                Err(_) => {
                    let mut data = self.lock()?;
                    let record = data
                        .registry
                        .records
                        .get_mut(&sequence)
                        .ok_or(ErrorCode::Uncertain)?;
                    if record.outcome.is_none() {
                        record.outcome = Some("refused-output-validation".into());
                    }
                    record.evidence = Some(evidence);
                    record.state = State::Settling;
                    persist(Path::new(&self.config.root), &data.registry)?;
                }
            }
        } else {
            let mut data = self.lock()?;
            let record = data
                .registry
                .records
                .get_mut(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            if record.outcome.is_none() {
                record.outcome = Some(outcome_name(outcome).into());
            }
            record.evidence = Some(evidence);
            record.state = State::Settling;
            persist(Path::new(&self.config.root), &data.registry)?;
        }
        self.settle_tail(sequence)?;
        Ok(())
    }

    fn wait_settled(self: &Arc<Self>, sequence: u64) -> Result<(), ErrorCode> {
        loop {
            let record = self.record(sequence)?;
            if let Some(accepted) = record.validation_ack {
                {
                    let mut data = self.lock()?;
                    let record = data
                        .registry
                        .records
                        .get_mut(&sequence)
                        .ok_or(ErrorCode::Uncertain)?;
                    if record.outcome.is_none() {
                        record.outcome = Some(if accepted {
                            "completed".into()
                        } else {
                            "refused-output-validation".into()
                        });
                    }
                    record.state = State::Settling;
                    persist(Path::new(&self.config.root), &data.registry)?;
                }
                self.settle_tail(sequence)?;
                return Ok(());
            }
            if record.cancellation_requested {
                return self.settle(sequence, Outcome::Cancelled).map(|_| ());
            }
            if now()? >= record.deadline_unix_ms {
                return self.settle(sequence, Outcome::Deadline).map(|_| ());
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

    /// Terminal settlement: stop the worker, capture the terminal resource
    /// evidence, remove the attempt boundary, and settle the receipt durably.
    fn settle_tail(&self, sequence: u64) -> Result<ResultBody, ErrorCode> {
        let mut record = self.record(sequence)?;
        if self.live(&record).is_ok_and(|live| live.running) {
            self.stop(&record)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.live(&record)?.running {
                if Instant::now() >= deadline {
                    return Err(ErrorCode::Uncertain);
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        if record.unit_invocation.is_some() {
            let evidence = self.terminal_evidence(&record)?;
            let mut data = self.lock()?;
            let record = data
                .registry
                .records
                .get_mut(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.evidence = Some(evidence);
            persist(Path::new(&self.config.root), &data.registry)?;
            drop(data);
        } else if record.evidence.is_none() {
            let mut data = self.lock()?;
            let record = data
                .registry
                .records
                .get_mut(&sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.evidence = Some(Evidence {
                peak_bytes: 0,
                exit_code: None,
                docker_oom_killed: None,
                attempt_before: None,
                attempt_after: None,
                parent_before: None,
                parent_after: None,
                populated: Some(false),
                terminal_snapshot: None,
            });
            persist(Path::new(&self.config.root), &data.registry)?;
            drop(data);
        }
        self.cleanup(&mut record)?;
        let body = {
            let mut data = self.lock()?;
            let incarnation = data.registry.incarnation.clone();
            let record = {
                let record = data
                    .registry
                    .records
                    .get_mut(&sequence)
                    .ok_or(ErrorCode::Uncertain)?;
                record.cleanup = Cleanup::Complete;
                record.state = State::Settled;
                record.settled_at_unix_ms = Some(now()?);
                record.clone()
            };
            if data.registry.active == Some(sequence) {
                data.registry.active = None;
            }
            let body = record.result_body(&incarnation);
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
            if self.live(&record).is_ok_and(|live| live.running) {
                self.stop(&record)?;
            }
            let requested = if record.cancellation_requested {
                Outcome::Cancelled
            } else if now()? >= record.deadline_unix_ms {
                Outcome::Deadline
            } else {
                Outcome::Interrupted
            };
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
        let path = record
            .workspace(Path::new(&self.config.root))
            .join("work")
            .join(OUTPUT_NAME);
        crate::photo_tiff::validate(&path, self.config.output_bytes_max).is_ok_and(|found| {
            found.size == identity.size
                && found.sha256 == identity.sha256
                && found.width == identity.width
                && found.height == identity.height
        })
    }
}

// Host boundary --------------------------------------------------------

fn docker(config: &Config, args: &[String]) -> Result<String, ErrorCode> {
    let mut fixed = vec![
        "--host".into(),
        "unix:///var/run/docker.sock".into(),
        "--config".into(),
        format!("{}/docker-client", config.root),
    ];
    fixed.extend_from_slice(args);
    backend::command("/usr/bin/docker", &fixed)
}

/// Resolve and verify the pinned worker image. The image entrypoint and the
/// configured bundle label bind the execution identity to the configuration.
fn inspect_image(config: &Config) -> Result<String, ErrorCode> {
    let info: Value = serde_json::from_str(&docker(
        config,
        &backend::strings(&["info", "--format", "{{json .}}"]),
    )?)
    .map_err(|_| ErrorCode::Unavailable)?;
    if info["CgroupDriver"] != "systemd"
        || info["CgroupVersion"] != "2"
        || info["SecurityOptions"].as_array().is_none_or(|options| {
            options.iter().any(|option| {
                option
                    .as_str()
                    .is_some_and(|text| text.contains("rootless") || text.contains("userns"))
            })
        })
    {
        return Err(ErrorCode::Unavailable);
    }
    let image: Value = serde_json::from_str(&docker(
        config,
        &backend::strings(&["image", "inspect", "--format", "{{json .}}", &config.image]),
    )?)
    .map_err(|_| ErrorCode::Unavailable)?;
    let id = image["Id"].as_str().ok_or(ErrorCode::Unavailable)?;
    if !id.strip_prefix("sha256:").is_some_and(|id| hex(id, 64))
        || image["Config"]["Entrypoint"] != serde_json::json!([WORKER])
        || image["Config"]["Labels"]["slipstream.processing.photo.bundle"] != config.bundle
    {
        return Err(ErrorCode::Unavailable);
    }
    Ok(id.to_owned())
}

impl PhotoExecutor {
    fn systemctl(&self, args: &[String]) -> Result<String, ErrorCode> {
        let mut fixed = vec!["--system".into()];
        fixed.extend_from_slice(args);
        backend::command("/usr/bin/systemctl", &fixed)
    }

    fn property(&self, unit: &str, property: &str) -> Result<String, ErrorCode> {
        self.systemctl(&backend::strings(&[
            "show",
            unit,
            "--property",
            property,
            "--value",
        ]))
    }

    fn parent_path(&self) -> PathBuf {
        Path::new(CGROUP).join(parent_unit(&self.config.instance))
    }

    fn container_name(&self, record: &PhotoRecord) -> String {
        format!("slipstream-processing-{}", record.launch_id)
    }

    fn mounts(&self, record: &PhotoRecord) -> Vec<(&'static str, PathBuf, bool)> {
        let base = record.workspace(Path::new(&self.config.root));
        vec![
            ("/control", base.join("control"), false),
            ("/input", base.join("source"), false),
            ("/work", base.join("work"), true),
        ]
    }

    fn check_caller(&self, pid: u32) -> Result<(), ErrorCode> {
        let parent = self.parent_path();
        for pid in [pid, std::process::id()] {
            let path = backend::process_cgroup(pid)?;
            if path.starts_with(&parent) {
                return Err(ErrorCode::Unavailable);
            }
            for ancestor in parent.ancestors() {
                if path.starts_with(ancestor) {
                    backend::require_unlimited_ancestor(ancestor, ancestor == Path::new(CGROUP))?;
                }
                if ancestor == Path::new(CGROUP) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn verify_parent(&self, identity: Option<&ParentIdentity>) -> Result<(), ErrorCode> {
        let path = self.parent_path();
        if let Some(identity) = identity {
            if fs::metadata(&path).map_err(|_| ErrorCode::Uncertain)?.ino() != identity.inode
                || self.property(&parent_unit(&self.config.instance), "InvocationID")?
                    != identity.invocation
                || !backend::read(&path.join("cgroup.procs"))?.is_empty()
            {
                return Err(ErrorCode::Uncertain);
            }
            return Ok(());
        }
        // Inventory queries do not synthesize a unit and cannot grant ownership.
        if path.exists()
            || !self
                .systemctl(&backend::strings(&[
                    "list-units",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "--no-pager",
                    &parent_unit(&self.config.instance),
                ]))?
                .is_empty()
        {
            return Err(ErrorCode::Uncertain);
        }
        Ok(())
    }

    fn prepare_parent(
        &self,
        previous: Option<&ParentIdentity>,
    ) -> Result<ParentIdentity, ErrorCode> {
        self.check_caller(std::process::id())?;
        self.verify_parent(previous)?;
        let unit = parent_unit(&self.config.instance);
        let limits = self.limits();
        self.systemctl(&backend::strings(&[
            "set-property",
            "--runtime",
            &unit,
            &format!("MemoryMax={}", self.config.memory_bytes),
            "MemorySwapMax=0",
            &format!("TasksMax={}", limits.tasks),
            &format!("CPUQuota={}%", limits.cpu_quota_us / 1000),
        ]))?;
        self.systemctl(&backend::strings(&["start", &unit]))?;
        if self.property(&unit, "StopWhenUnneeded")? != "no" {
            return Err(ErrorCode::Unavailable);
        }
        if !self.limits_match(&self.parent_path())? {
            return Err(ErrorCode::Unavailable);
        }
        let identity = ParentIdentity {
            invocation: self.property(&unit, "InvocationID")?,
            inode: fs::metadata(self.parent_path())
                .map_err(|_| ErrorCode::Uncertain)?
                .ino(),
        };
        if !hex(&identity.invocation, 32) || previous.is_some_and(|previous| previous != &identity)
        {
            return Err(ErrorCode::Uncertain);
        }
        self.verify_parent(Some(&identity))?;
        Ok(identity)
    }

    fn admission_ready(&self, identity: Option<&ParentIdentity>) -> Result<(), ErrorCode> {
        // The configured control reserve and shared-ancestor headroom are
        // part of admission: an unmet or unmeasured boundary keeps the
        // reported capability blocked, never falsely available.
        Headroom::measure(&self.config)?.satisfied(&self.config)?;
        let identity = identity.ok_or(ErrorCode::Unavailable)?;
        if fs::metadata(self.parent_path())
            .map_err(|_| ErrorCode::Unavailable)?
            .ino()
            != identity.inode
            || !backend::read(&self.parent_path().join("cgroup.procs"))?.is_empty()
        {
            return Err(ErrorCode::Unavailable);
        }
        if self.limits_match(&self.parent_path())? {
            Ok(())
        } else {
            Err(ErrorCode::Unavailable)
        }
    }

    fn limits_match(&self, path: &Path) -> Result<bool, ErrorCode> {
        let limits = self.limits();
        Ok(
            backend::read(&path.join("memory.max"))? == limits.memory_bytes.to_string()
                && backend::read(&path.join("memory.swap.max"))? == "0"
                && backend::read(&path.join("cpu.max"))?
                    == format!("{} {}", limits.cpu_quota_us, limits.cpu_period_us)
                && backend::read(&path.join("pids.max"))? == limits.tasks.to_string(),
        )
    }

    fn scan_unowned(&self, records: &[PhotoRecord]) -> Result<(), ErrorCode> {
        let unit = |record: &PhotoRecord| attempt_unit(&self.config.instance, &record.launch_id);
        if self.parent_path().exists() {
            for entry in fs::read_dir(self.parent_path()).map_err(|_| ErrorCode::Uncertain)? {
                let entry = entry.map_err(|_| ErrorCode::Uncertain)?;
                if entry
                    .file_type()
                    .map_err(|_| ErrorCode::Uncertain)?
                    .is_dir()
                    && !records.iter().any(|record| {
                        record.state != State::Settled
                            && entry.file_name() == unit(record).as_str()
                            && self.verify_unit(record).is_ok()
                    })
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        let names = self.systemctl(&backend::strings(&[
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            "--no-pager",
            &format!("slipstreamprocessing{}-*.slice", self.config.instance),
        ]))?;
        for line in names.lines() {
            let name = line.split_whitespace().next().ok_or(ErrorCode::Uncertain)?;
            if !records
                .iter()
                .any(|record| record.state != State::Settled && unit(record) == name)
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        let ids = docker(
            &self.config,
            &backend::strings(&[
                "ps",
                "--all",
                "--no-trunc",
                "--quiet",
                "--filter",
                &format!(
                    "label=slipstream.processing.instance={}",
                    self.config.instance
                ),
            ]),
        )?;
        for id in ids.lines() {
            if !records.iter().any(|record| {
                record.container_id.as_deref() == Some(id) && record.state != State::Settled
            }) {
                return Err(ErrorCode::Uncertain);
            }
        }
        Ok(())
    }

    fn discover(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if record.manager_pending == Some(ManagerPhase::SliceStop) {
            // A pending slice stop decides whether the attempt boundary still
            // exists, and it is cleared only with its confirmed return. A
            // restart cannot assume its effect, so it stays ambiguous and
            // requires operator reconciliation.
            return Err(ErrorCode::Uncertain);
        }
        if matches!(
            record.manager_pending,
            Some(ManagerPhase::Create | ManagerPhase::CreateReturned)
        ) && record.container_id.is_none()
        {
            let ids = docker(
                &self.config,
                &backend::strings(&[
                    "ps",
                    "--all",
                    "--no-trunc",
                    "--quiet",
                    "--filter",
                    &format!("label=slipstream.processing.launch={}", record.launch_id),
                ]),
            )?;
            let ids: Vec<_> = ids.lines().collect();
            match ids.as_slice() {
                [id] => {
                    let candidate = record.container_id.clone();
                    record.container_id = Some((*id).to_owned());
                    if self.owned_container(record).is_err() {
                        record.container_id = candidate;
                        return Err(ErrorCode::Uncertain);
                    }
                }
                [] => {}
                _ => return Err(ErrorCode::Uncertain),
            }
        }
        let path = self
            .parent_path()
            .join(attempt_unit(&self.config.instance, &record.launch_id));
        if path.exists() {
            self.verify_unit(record)?;
        }
        let mount =
            backend::mount_identity(&record.workspace(Path::new(&self.config.root)).join("work"))?;
        if mount.is_some() && mount != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        // The remaining recorded phases mark an effect whose outcome the
        // observed slice, mount and container identities settle here: the
        // attempt boundary is exactly what the launcher recorded, and every
        // later step fails closed on the actual container state instead of on
        // this marker. Leaving them set would block settlement and cleanup
        // forever after a worker that failed before its release gate, because
        // the release-gate pause cannot complete on a worker that already
        // exited.
        record.manager_pending = None;
        Ok(())
    }

    fn verify_unit(&self, record: &PhotoRecord) -> Result<PathBuf, ErrorCode> {
        let name = attempt_unit(&self.config.instance, &record.launch_id);
        let path = self.parent_path().join(&name);
        backend::verify_slice_phase(
            record.manager_pending,
            record.stop_confirmed,
            || {
                slice::verify(
                    &name,
                    &path,
                    record
                        .unit_invocation
                        .as_deref()
                        .ok_or(ErrorCode::Uncertain)?,
                    record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
                )
            },
            || {
                slice::wait_absent(
                    &name,
                    &path,
                    record
                        .unit_invocation
                        .as_deref()
                        .ok_or(ErrorCode::Uncertain)?,
                    record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
                )
            },
        )?;
        Ok(path)
    }

    fn owned_container(&self, record: &PhotoRecord) -> Result<Value, ErrorCode> {
        let id = record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?;
        if !hex(id, 64) {
            return Err(ErrorCode::Uncertain);
        }
        let value: Value = serde_json::from_str(&docker(
            &self.config,
            &backend::strings(&["inspect", "--format", "{{json .}}", id]),
        )?)
        .map_err(|_| ErrorCode::Uncertain)?;
        if value["Id"] != id
            || value["Image"] != self.image_id
            || value["Name"] != format!("/{}", self.container_name(record))
            || value["Config"]["Labels"]["slipstream.processing.instance"] != self.config.instance
            || value["Config"]["Labels"]["slipstream.processing.launch"] != record.launch_id
            || value["Config"]["Labels"]["slipstream.processing.incarnation"] != record.incarnation
            || value["HostConfig"]["CgroupParent"]
                != attempt_unit(&self.config.instance, &record.launch_id)
            || value["Config"]["User"] != "1000:1000"
            || value["Config"]["Entrypoint"] != serde_json::json!([WORKER])
            || value["Config"]["Cmd"]
                != serde_json::json!([
                    protocol::PHOTO_WORKLOAD,
                    record.launch_id,
                    record.deadline_unix_ms.to_string()
                ])
            || value["HostConfig"]["LogConfig"]["Type"] != "none"
            || value["HostConfig"]["Privileged"] != false
            || value["HostConfig"]["ReadonlyRootfs"] != true
            || value["HostConfig"]["NetworkMode"] != "none"
            || value["HostConfig"]["PidMode"] != ""
            || value["HostConfig"]["CgroupnsMode"] != "private"
            || value["HostConfig"]["CapDrop"] != serde_json::json!(["ALL"])
            || !value["HostConfig"]["CapAdd"].is_null()
            || value["HostConfig"]["SecurityOpt"] != serde_json::json!(["no-new-privileges:true"])
            || value["HostConfig"]["Memory"] != self.config.memory_bytes
            || value["HostConfig"]["MemorySwap"] != self.config.memory_bytes
            || value["HostConfig"]["PidsLimit"] != u64::from(self.config.tasks)
            || value["HostConfig"]["NanoCpus"] != self.config.cpu_quota_us * 10000
            || value["Config"]["Tty"] != false
            || value["Config"]["OpenStdin"] != false
        {
            return Err(ErrorCode::Uncertain);
        }
        let mounts = value["Mounts"].as_array().ok_or(ErrorCode::Uncertain)?;
        let expected = self.mounts(record);
        if mounts.len() != expected.len() {
            return Err(ErrorCode::Uncertain);
        }
        for (destination, source, writable) in expected {
            if mounts
                .iter()
                .filter(|mount| {
                    mount["Type"] == "bind"
                        && mount["Destination"] == destination
                        && mount["Source"]
                            .as_str()
                            .is_some_and(|value| Path::new(value) == source)
                        && mount["RW"] == writable
                        && mount["Propagation"] == "rprivate"
                })
                .count()
                != 1
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        Ok(value)
    }

    fn live(&self, record: &PhotoRecord) -> Result<Live, ErrorCode> {
        let value = self.owned_container(record)?;
        Ok(Live {
            running: value["State"]["Running"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
            pid: value["State"]["Pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or(ErrorCode::Uncertain)?,
            exit_code: backend::observed_exit(&value["State"])?,
            oom: value["State"]["OOMKilled"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
        })
    }

    /// Create the fresh retained attempt boundary before worker bootstrap:
    /// the attempt slice, the bounded private tmpfs, the release gate and the
    /// pinned paused worker with verified placement and limits.
    fn provision(&self, record: &mut PhotoRecord) -> Result<File, ErrorCode> {
        let name = attempt_unit(&self.config.instance, &record.launch_id);
        let expected = self.parent_path().join(&name);
        let workspace = record.workspace(Path::new(&self.config.root));
        record.manager_pending = Some(ManagerPhase::Slice);
        self.update(record)?;
        let (invocation, inode) = slice::create(&name, &expected, &self.limits())?;
        record.unit_invocation = Some(invocation);
        record.cgroup_inode = Some(inode);
        if !self.limits_match(&expected)? {
            return Err(ErrorCode::Unavailable);
        }
        record.manager_pending = None;
        self.update(record)?;
        backend::create_private_directory(&workspace)?;
        let control = workspace.join("control");
        let work = workspace.join("work");
        backend::create_control_directory(&control)?;
        fs::create_dir(&work).map_err(|_| ErrorCode::Unavailable)?;
        record.manager_pending = Some(ManagerPhase::Mount);
        self.update(record)?;
        backend::command(
            "/usr/bin/mount",
            &backend::strings(&[
                "-t",
                "tmpfs",
                "-o",
                &format!(
                    "size={},nr_inodes={},noswap,nodev,nosuid,noexec,uid=1000,gid=1000,mode=0700",
                    self.limits().storage_bytes,
                    self.limits().storage_inodes,
                ),
                &format!("slipstream-{}", record.launch_id),
                work.to_str().ok_or(ErrorCode::Unavailable)?,
            ]),
        )?;
        record.mount_id = Some(backend::mount_identity(&work)?.ok_or(ErrorCode::Uncertain)?);
        record.manager_pending = None;
        self.update(record)?;
        let gate = backend::create_native_gate(&control.join("gate"))?;
        write_grant(record, &control)?;
        let mut args = backend::strings(&[
            "create",
            "--pull",
            "never",
            "--name",
            &self.container_name(record),
            "--label",
            &format!("slipstream.processing.instance={}", self.config.instance),
            "--label",
            &format!("slipstream.processing.launch={}", record.launch_id),
            "--label",
            &format!("slipstream.processing.incarnation={}", record.incarnation),
            "--cgroup-parent",
            &name,
            "--cgroupns",
            "private",
            "--network",
            "none",
            "--user",
            "1000:1000",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges:true",
            "--log-driver",
            "none",
            "--memory",
            &self.config.memory_bytes.to_string(),
            "--memory-swap",
            &self.config.memory_bytes.to_string(),
            "--pids-limit",
            &self.config.tasks.to_string(),
            "--cpus",
            &(self.config.cpu_quota_us / 100_000).to_string(),
        ]);
        for (destination, source, writable) in self.mounts(record) {
            args.extend(backend::strings(&[
                "--mount",
                &format!(
                    "type=bind,source={},target={destination}{}",
                    source.display(),
                    if writable { "" } else { ",readonly" }
                ),
            ]));
        }
        args.extend(backend::strings(&[
            &self.image_id,
            protocol::PHOTO_WORKLOAD,
            &record.launch_id,
            &record.deadline_unix_ms.to_string(),
        ]));
        record.manager_pending = Some(ManagerPhase::Create);
        self.update(record)?;
        let id = docker(&self.config, &args)?;
        if !hex(&id, 64) {
            return Err(ErrorCode::Uncertain);
        }
        record.manager_pending = Some(ManagerPhase::CreateReturned);
        self.update(record)?;
        record.container_id = Some(id);
        record.manager_pending = None;
        self.update(record)?;
        record.manager_pending = Some(ManagerPhase::Start);
        self.update(record)?;
        docker(
            &self.config,
            &backend::strings(&[
                "start",
                record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?,
            ]),
        )?;
        record.manager_pending = None;
        self.update(record)?;
        record.manager_pending = Some(ManagerPhase::Pause);
        self.update(record)?;
        docker(
            &self.config,
            &backend::strings(&[
                "pause",
                record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?,
            ]),
        )?;
        record.manager_pending = None;
        self.update(record)?;
        self.place(record, &expected)?;
        record.image_id = Some(self.image_id.clone());
        record.phase = Phase::Provisioned;
        self.update(record)?;
        Ok(gate)
    }

    /// Verify the frozen worker placement: its cgroup scope, the paused
    /// frozen state, the delegated workload leaf and the enforced limits.
    fn place(&self, record: &PhotoRecord, expected: &Path) -> Result<(), ErrorCode> {
        let live = self.live(record)?;
        if !live.running || live.pid == 0 {
            return Err(ErrorCode::Unavailable);
        }
        let id = record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?;
        let scope = backend::process_cgroup(live.pid)?;
        if scope.parent() != Some(expected)
            || scope.file_name().and_then(|value| value.to_str())
                != Some(&format!("docker-{id}.scope"))
        {
            return Err(ErrorCode::Unavailable);
        }
        // The release-gate pause freezes the worker's own cgroup scope; the
        // attempt slice above it stays unfrozen, so the frozen state is read
        // from the scope, exactly as the reference placement check does.
        if !backend::read(&scope.join("cgroup.events"))?
            .lines()
            .any(|line| line == "frozen 1")
            || self.owned_container(record)?["State"]["Paused"] != true
        {
            return Err(ErrorCode::Uncertain);
        }
        // pidfd and the opened proc directory detect disappearance without
        // retargeting readback to a reused PID.
        let process =
            File::open(format!("/proc/{}", live.pid)).map_err(|_| ErrorCode::Uncertain)?;
        // SAFETY: pidfd_open accepts this checked positive PID and flags zero.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, live.pid, 0) };
        if descriptor < 0 {
            return Err(ErrorCode::Uncertain);
        }
        // SAFETY: successful pidfd_open returns a fresh descriptor owned by this call.
        let pidfd = unsafe { OwnedFd::from_raw_fd(descriptor as i32) };
        let proc_path = PathBuf::from(format!("/proc/self/fd/{}", process.as_raw_fd()));
        let placed = backend::read(&proc_path.join("cgroup"))?
            == format!(
                "0::{}",
                scope
                    .strip_prefix(CGROUP)
                    .map_err(|_| ErrorCode::Uncertain)?
                    .display()
            )
            .replace("0::", "0::/");
        if !placed {
            return Err(ErrorCode::Unavailable);
        }
        pidfd_alive(&pidfd)?;
        if !self.limits_match(&scope)? {
            return Err(ErrorCode::Unavailable);
        }
        let leaf = scope.join("workload");
        fs::create_dir(&leaf).map_err(|_| ErrorCode::Unavailable)?;
        write_cgroup(&leaf.join("cgroup.procs"), &live.pid.to_string())?;
        backend::delegate_workload_controllers(&scope, &leaf)?;
        let limits = self.limits();
        for (key, value) in [
            ("memory.max", limits.memory_bytes.to_string()),
            ("memory.swap.max", "0".into()),
            ("memory.oom.group", "1".into()),
            (
                "cpu.max",
                format!("{} {}", limits.cpu_quota_us, limits.cpu_period_us),
            ),
            ("pids.max", limits.tasks.to_string()),
        ] {
            write_cgroup(&leaf.join(key), &value)?;
        }
        if !self.limits_match(&leaf)? {
            return Err(ErrorCode::Unavailable);
        }
        let placed = backend::read(&leaf.join("memory.oom.group"))? == "1"
            && backend::read(&leaf.join("cgroup.procs"))? == live.pid.to_string()
            && backend::process_cgroup(live.pid)? == leaf;
        if !placed {
            return Err(ErrorCode::Unavailable);
        }
        self.owned_container(record)?;
        Ok(())
    }

    fn release(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        self.owned_container(record)?;
        let id = record.container_id.clone().ok_or(ErrorCode::Uncertain)?;
        record.manager_pending = Some(ManagerPhase::Unpause);
        self.update(record)?;
        docker(&self.config, &backend::strings(&["unpause", &id]))?;
        record.manager_pending = None;
        record.released = true;
        self.update(record)
    }

    fn stop(&self, record: &PhotoRecord) -> Result<(), ErrorCode> {
        let Some(id) = record.container_id.clone() else {
            return Ok(());
        };
        if self.live(record)?.running {
            self.verify_unit(record)?;
            docker(
                &self.config,
                &backend::strings(&["kill", "--signal", "KILL", &id]),
            )?;
        }
        Ok(())
    }

    fn terminal_evidence(&self, record: &PhotoRecord) -> Result<Evidence, ErrorCode> {
        let path = self.verify_unit(record)?;
        let has_container = record.container_id.is_some();
        let live = if has_container {
            Some(self.live(record)?)
        } else {
            None
        };
        if live.as_ref().is_some_and(|live| live.running) {
            return Err(ErrorCode::Uncertain);
        }
        if !backend::cgroup_unpopulated(&path)? {
            return Err(ErrorCode::Uncertain);
        }
        let peak_bytes = backend::read(&path.join("memory.peak"))?
            .parse()
            .map_err(|_| ErrorCode::Uncertain)?;
        Ok(Evidence {
            peak_bytes,
            exit_code: live.as_ref().and_then(|live| live.exit_code),
            docker_oom_killed: live
                .as_ref()
                .and_then(|live| live.exit_code.map(|_| live.oom)),
            attempt_before: record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.attempt_before.clone()),
            attempt_after: Some(backend::events(&path)?),
            parent_before: record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.parent_before.clone()),
            parent_after: Some(backend::events(&self.parent_path())?),
            populated: Some(false),
            terminal_snapshot: None,
        })
    }

    fn worker_outcome(&self, record: &PhotoRecord) -> Result<Option<Outcome>, ErrorCode> {
        let path = record
            .workspace(Path::new(&self.config.root))
            .join("work")
            .join(RESULT_NAME);
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ErrorCode::Uncertain),
        };
        let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
        if !metadata.is_file() || metadata.len() != 4096 {
            return Err(ErrorCode::Uncertain);
        }
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| ErrorCode::Uncertain)?;
        if bytes.len() != 4096 {
            return Err(ErrorCode::Uncertain);
        }
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        if bytes[end..].iter().any(|byte| *byte != 0) {
            return Err(ErrorCode::Uncertain);
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ResultFile {
            launch_id: String,
            outcome: Outcome,
        }
        let value: ResultFile = match serde_json::from_slice(&bytes[..end]) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if value.launch_id != record.launch_id {
            return Err(ErrorCode::Uncertain);
        }
        Ok(Some(value.outcome))
    }

    fn cleanup(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if record.manager_pending.is_some() {
            return Err(ErrorCode::Uncertain);
        }
        if let Some(id) = record.container_id.clone() {
            let ids = docker(
                &self.config,
                &backend::strings(&[
                    "ps",
                    "--all",
                    "--no-trunc",
                    "--quiet",
                    "--filter",
                    &format!("id={id}"),
                ]),
            )?;
            if !ids.is_empty() {
                if self.live(record)?.running {
                    return Err(ErrorCode::Uncertain);
                }
                docker(&self.config, &backend::strings(&["rm", &id]))?;
            }
        }
        let workspace = record.workspace(Path::new(&self.config.root));
        let work = workspace.join("work");
        if let Some(current) = backend::mount_identity(&work)? {
            if Some(current) != record.mount_id {
                return Err(ErrorCode::Uncertain);
            }
            backend::command(
                "/usr/bin/umount",
                &backend::strings(&[work.to_str().ok_or(ErrorCode::Uncertain)?]),
            )?;
            if backend::mount_identity(&work)?.is_some() {
                return Err(ErrorCode::Uncertain);
            }
        }
        if workspace.exists() {
            fs::remove_dir_all(&workspace).map_err(|_| ErrorCode::Uncertain)?;
        }
        if record.unit_invocation.is_some() && !record.stop_confirmed {
            self.verify_unit(record)?;
            record.manager_pending = Some(ManagerPhase::SliceStop);
            self.update(record)?;
            self.systemctl(&backend::strings(&[
                "stop",
                &attempt_unit(&self.config.instance, &record.launch_id),
            ]))?;
            record.stop_confirmed = true;
            record.manager_pending = None;
            self.update(record)?;
        }
        let path = self
            .parent_path()
            .join(attempt_unit(&self.config.instance, &record.launch_id));
        if let Some(invocation) = &record.unit_invocation {
            if !record.stop_confirmed {
                return Err(ErrorCode::Uncertain);
            }
            slice::wait_absent(
                &attempt_unit(&self.config.instance, &record.launch_id),
                &path,
                invocation,
                record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
            )?;
        } else {
            slice::never_created_absent(
                &attempt_unit(&self.config.instance, &record.launch_id),
                &path,
            )?;
        }
        Ok(())
    }
}

/// Write the bounded engine grant into the read-only control mount before
/// the container exists. The recipe payload reaches the worker only through
/// this launcher-owned file; the worker verifies its launch binding.
fn write_grant(record: &PhotoRecord, control: &Path) -> Result<(), ErrorCode> {
    let grant = serde_json::json!({
        "version": 1,
        "kind": "photo-development-grant",
        "launch_id": record.launch_id,
        "workload": protocol::PHOTO_WORKLOAD,
        "exposure_milli_ev": record.recipe.exposure_milli_ev,
        "white_balance_mode": record.recipe.white_balance_mode,
        "profile_id": record.source.profile_id,
        "icc_asset_sha256": crate::photo::ICC_ASSET_SHA256,
    });
    let bytes = serde_json::to_vec(&grant).map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > 16 * 1024 {
        return Err(ErrorCode::Uncertain);
    }
    let path = control.join("grant.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| ErrorCode::Unavailable)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ErrorCode::Unavailable)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
        .map_err(|_| ErrorCode::Unavailable)?;
    File::open(control)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ErrorCode::Unavailable)
}

fn pidfd_alive(pidfd: &OwnedFd) -> Result<(), ErrorCode> {
    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor is a live pidfd and the single pollfd buffer is valid.
    if unsafe { libc::poll(&mut descriptor, 1, 0) } != 0 || descriptor.revents != 0 {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

fn write_cgroup(path: &Path, value: &str) -> Result<(), ErrorCode> {
    fs::write(path, value).map_err(|_| ErrorCode::Unavailable)
}

// Admission ------------------------------------------------------------

/// Measured control-path storage and shared-ancestor memory headroom. Both
/// must cover their configured reserve before an intent is persisted or a
/// worker is released (`design/processing-photo-protocol.md` admission
/// ordering step 3).
#[derive(Clone, Copy, Debug)]
struct Headroom {
    control_free_bytes: u64,
    ancestor_headroom_bytes: u64,
}

impl Headroom {
    /// Measure the actual filesystem supply at the control root and the
    /// memory headroom of the shared ancestor above the processing subtree.
    fn measure(config: &Config) -> Result<Self, ErrorCode> {
        Ok(Self {
            control_free_bytes: control_free_bytes(Path::new(&config.root))?,
            ancestor_headroom_bytes: shared_ancestor_headroom()?,
        })
    }

    /// An unmet or unmeasurable reserve leaves the capability unavailable:
    /// admission is refused and no start intent is persisted, so no worker
    /// is ever released onto an unqualified boundary.
    fn satisfied(&self, config: &Config) -> Result<(), ErrorCode> {
        if self.control_free_bytes < config.control_reserve_bytes
            || self.ancestor_headroom_bytes < config.shared_ancestor_headroom_bytes
        {
            return Err(ErrorCode::Unavailable);
        }
        Ok(())
    }
}

/// Bytes currently free on the filesystem that carries the control root.
fn control_free_bytes(root: &Path) -> Result<u64, ErrorCode> {
    let file = File::open(root).map_err(|_| ErrorCode::Unavailable)?;
    // SAFETY: fstatvfs receives the live descriptor and a correctly sized output struct.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatvfs(file.as_raw_fd(), &mut stat) } != 0 {
        return Err(ErrorCode::Unavailable);
    }
    let blocks = stat.f_bavail;
    let frsize = stat.f_frsize;
    blocks.checked_mul(frsize).ok_or(ErrorCode::Unavailable)
}

/// The memory headroom of the shared ancestor above the capped processing
/// subtree. A missing or unlimited ancestor limit defers to the host supply
/// reported by the kernel; a finite limit leaves `max - current` bytes. An
/// overcommitted ancestor has no measurable headroom and refuses admission.
fn shared_ancestor_headroom() -> Result<u64, ErrorCode> {
    let ancestor = match backend::read(&Path::new(CGROUP).join("memory.max")) {
        Ok(limit) if limit.trim() != "max" => {
            let limit: u64 = limit.trim().parse().map_err(|_| ErrorCode::Unavailable)?;
            let current: u64 = backend::read(&Path::new(CGROUP).join("memory.current"))?
                .trim()
                .parse()
                .map_err(|_| ErrorCode::Unavailable)?;
            limit.checked_sub(current).ok_or(ErrorCode::Unavailable)?
        }
        Ok(_) => u64::MAX,
        Err(_) => u64::MAX,
    };
    Ok(ancestor.min(meminfo_available_bytes()?))
}

/// The kernel's estimate of memory available for new work without swapping.
fn meminfo_available_bytes() -> Result<u64, ErrorCode> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|_| ErrorCode::Unavailable)?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("MemAvailable:") {
            let kilobytes: u64 = value
                .trim()
                .strip_suffix("kB")
                .ok_or(ErrorCode::Unavailable)?
                .trim()
                .parse()
                .map_err(|_| ErrorCode::Unavailable)?;
            return kilobytes.checked_mul(1024).ok_or(ErrorCode::Unavailable);
        }
    }
    Err(ErrorCode::Unavailable)
}

/// Perform every bounded admission check and persist the executor intent.
/// The descriptor copy runs afterwards, without the journal mutex held.
#[allow(clippy::too_many_arguments)]
fn begin_start(
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
    // bundle or policy outside the fixed authority has no admitted plan.
    if photo_profile::approved_profile(&source.profile_id).is_none() {
        return Err(ErrorCode::InvalidRequest);
    }
    if !EXPOSURE_MILLI_EV_RANGE.contains(&recipe.exposure_milli_ev) {
        return Err(ErrorCode::InvalidRequest);
    }
    if manifest_digest(request)? != *manifest_sha256 {
        return Err(ErrorCode::InvalidRequest);
    }
    if recipe_digest_of(recipe)? != *request_recipe_digest {
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
fn seal_source(
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
    let profile = photo_profile::approved_profile(&record.source.profile_id)
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
fn finalize_start(
    registry: &mut Registry,
    _config: &Config,
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
    if let Some(outcome) = refusal {
        let workspace = record.workspace(root);
        let _ = fs::remove_dir_all(workspace);
        {
            let record = registry
                .records
                .get_mut(sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.outcome = Some(outcome.into());
            record.state = State::Settled;
            record.settled_at_unix_ms = Some(now()?);
            record.cleanup = Cleanup::Complete;
        }
        registry.active = None;
        persist(root, registry)?;
        let _ = incarnation;
        return Err(if outcome == "interrupted" {
            ErrorCode::Unavailable
        } else {
            ErrorCode::InvalidRequest
        });
    }
    if record.cancellation_requested {
        let workspace = record.workspace(root);
        let _ = fs::remove_dir_all(workspace);
        let record = {
            let record = registry
                .records
                .get_mut(sequence)
                .ok_or(ErrorCode::Uncertain)?;
            record.outcome = Some("cancelled".into());
            record.state = State::Settled;
            record.settled_at_unix_ms = Some(now()?);
            record.cleanup = Cleanup::Complete;
            record.clone()
        };
        registry.active = None;
        persist(root, registry)?;
        return Ok(record.result_body(incarnation));
    }
    let record = {
        let record = registry
            .records
            .get_mut(sequence)
            .ok_or(ErrorCode::Uncertain)?;
        record.plan = Some(Plan::development_tiff());
        record.phase = Phase::Planned;
        record.clone()
    };
    persist(root, registry)?;
    Ok(record.result_body(incarnation))
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
        target: protocol::PHOTO_WORKLOAD.into(),
        size: identity.size,
        sha256,
    })
}

// Durable snapshot -----------------------------------------------------

fn record_ref<'a>(
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

fn record_ref_mut<'a>(
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

fn expire(registry: &mut Registry, time: u64, retention: u64) -> Result<(), ErrorCode> {
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
fn validate_registry(registry: &Registry, config: &Config) -> Result<(), ErrorCode> {
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
            || record.source.kind != "raw"
            || !photo::identifier(&record.source.profile_id, 64)
            || photo_profile::approved_profile(&record.source.profile_id).is_none()
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
            ) != Ok(record.manifest_sha256.clone())
            || recipe_digest(&record.recipe) != Ok(record.recipe_digest.clone())
            || record.accepted_at_unix_ms == 0
            || record.deadline_unix_ms < record.accepted_at_unix_ms
            || !hex(&record.launch_id, 32)
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
                if record.plan.as_ref() != Some(&Plan::development_tiff()) {
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

fn prepare_private_directory(path: &Path) -> Result<(), ErrorCode> {
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

fn load(root: &Path) -> Result<Option<Registry>, ErrorCode> {
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

fn persist(root: &Path, registry: &Registry) -> Result<(), ErrorCode> {
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

fn random_id() -> Result<String, ErrorCode> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| ErrorCode::Unavailable)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Restore the durable registry, or initialize a fresh one with a new
/// incarnation, exactly as a first start. An existing claim whose root has
/// no registry never reaches here: the shared claim path quarantines it.
fn restore_registry(root: &Path, config: &Config) -> Result<Registry, ErrorCode> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PHOTO_MODE, PHOTO_PROTOCOL_VERSION};
    use std::{
        os::unix::fs::OpenOptionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "slipstream-photo-exec-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&path)
            .unwrap();
        path
    }

    fn test_config(root: &Path) -> Config {
        Config {
            version: PHOTO_PROTOCOL_VERSION,
            mode: PHOTO_MODE.into(),
            instance: "0".repeat(32),
            root: root.display().to_string(),
            socket: root.join("launcher.sock").display().to_string(),
            // The production peer is Web UID 1000; the focused tests
            // authenticate their actual peer so they run under any CI UID.
            peer_uid: unsafe { libc::geteuid() },
            image: format!("sha256:{}", "1".repeat(64)),
            bundle: "2".repeat(64),
            policy: "3".repeat(64),
            source_bytes_max: 4096,
            staged_storage_bytes_max: 8192,
            staged_storage_inodes_max: 64,
            output_bytes_max: 8192,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            cpu_quota_us: 400_000,
            tasks: 256,
            swap_bytes: 0,
            control_reserve_bytes: 1024 * 1024,
            shared_ancestor_headroom_bytes: 1024 * 1024,
            receipt_retention_seconds: 86_400,
        }
    }

    /// Headroom comfortably above every configured test reserve.
    fn satisfied_headroom() -> Headroom {
        Headroom {
            control_free_bytes: 1 << 40,
            ancestor_headroom_bytes: 1 << 40,
        }
    }

    fn empty_registry(instance: &str) -> Registry {
        Registry {
            version: 1,
            instance: instance.into(),
            incarnation: "a".repeat(32),
            watermark: 0,
            parent_pending: false,
            parent_identity: None,
            active: None,
            records: BTreeMap::new(),
        }
    }

    fn source_file(root: &Path, bytes: &[u8]) -> File {
        let path = root.join(format!(
            "staged-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        drop(file);
        // Seal the staged copy exactly like the service does.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
            .unwrap()
    }

    fn start_request(
        config: &Config,
        overrides: impl FnOnce(&mut photo::Request),
    ) -> photo::Request {
        let mut request = photo::Request::Start {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: config.instance.clone(),
            export_id: "export-1".into(),
            incarnation: "a".repeat(32),
            sequence: 1,
            policy: config.policy.clone(),
            bundle: config.bundle.clone(),
            workload: crate::protocol::PHOTO_WORKLOAD.into(),
            source: Source {
                kind: "raw".into(),
                profile_id: "sony-ilce-7rm5-arw".into(),
                size: 4,
                sha256: "4".repeat(64),
            },
            recipe: Recipe {
                exposure_milli_ev: 0,
                white_balance_mode: "as-shot".into(),
            },
            recipe_digest: String::new(),
            manifest_sha256: String::new(),
        };
        // Fill the canonical digests so a default request admits.
        {
            let photo::Request::Start {
                source: ref source_field,
                recipe: ref recipe_field,
                policy: ref policy_field,
                bundle: ref bundle_field,
                ref mut recipe_digest,
                ref mut manifest_sha256,
                ..
            } = request
            else {
                unreachable!()
            };
            *recipe_digest = recipe_digest_of(recipe_field).unwrap();
            *manifest_sha256 =
                manifest_digest_of(source_field, recipe_field, policy_field, bundle_field).unwrap();
        }
        overrides(&mut request);
        // Recompute nothing: overridden digests intentionally mismatch.
        request
    }

    fn with_canonical_digests(mut request: photo::Request) -> photo::Request {
        if let photo::Request::Start {
            ref source,
            ref recipe,
            ref policy,
            ref bundle,
            ref mut recipe_digest,
            ref mut manifest_sha256,
            ..
        } = request
        {
            *recipe_digest = recipe_digest_of(recipe).unwrap();
            *manifest_sha256 = manifest_digest_of(source, recipe, policy, bundle).unwrap();
        }
        request
    }

    fn record_for(sequence: u64, phase: Phase) -> PhotoRecord {
        let source = Source {
            kind: "raw".into(),
            profile_id: "sony-ilce-7rm5-arw".into(),
            size: 100,
            sha256: "4".repeat(64),
        };
        let recipe = Recipe {
            exposure_milli_ev: 1000,
            white_balance_mode: "as-shot".into(),
        };
        PhotoRecord {
            sequence,
            incarnation: "a".repeat(32),
            export_id: "export-1".into(),
            policy: "3".repeat(64),
            bundle: "2".repeat(64),
            state: if phase == Phase::OutputReady {
                State::Settling
            } else {
                State::Accepted
            },
            outcome: None,
            manifest_sha256: manifest_digest_of(
                &source,
                &recipe,
                "3".repeat(64).as_str(),
                "2".repeat(64).as_str(),
            )
            .unwrap(),
            recipe_digest: recipe_digest_of(&recipe).unwrap(),
            source,
            recipe,
            plan: (phase != Phase::Intent).then(|| Plan {
                workload: crate::protocol::PHOTO_WORKLOAD.into(),
                steps: vec!["develop".into()],
                output: crate::protocol::PHOTO_WORKLOAD.into(),
            }),
            phase,
            launch_id: "b".repeat(32),
            image_id: Some(format!("sha256:{}", "1".repeat(64))),
            unit_invocation: None,
            cgroup_inode: None,
            mount_id: None,
            container_id: None,
            released: false,
            cancellation_requested: false,
            accepted_at_unix_ms: 1,
            deadline_unix_ms: u64::MAX,
            manager_pending: None,
            stop_confirmed: false,
            evidence: None,
            output: None,
            output_transferred: false,
            validation_ack: None,
            cleanup: Cleanup::Pending,
            settled_at_unix_ms: None,
        }
    }

    fn executor_with(root: &Path, mut registry: Registry) -> Arc<PhotoExecutor> {
        let config = test_config(root);
        if registry.instance != config.instance {
            registry.instance = config.instance.clone();
        }
        for name in ["attempts", "docker-client"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(root.join(name))
                .unwrap();
        }
        persist(root, &registry).unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(root.join("owner.lock"))
            .unwrap();
        Arc::new(PhotoExecutor {
            config,
            data: Mutex::new(Data {
                registry,
                available: true,
            }),
            image_id: format!("sha256:{}", "1".repeat(64)),
            _lock: lock.try_clone().expect("owner lock handle"),
            _instance_claim: lock,
        })
    }

    #[test]
    fn descriptor_size_hash_and_identity_mismatch_settle_the_intent_without_a_worker() {
        let root = temp_dir("mismatch");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);

        // The declared digest never matches the sealed copy.
        let request = with_canonical_digests(start_request(&config, |_| {}));
        let descriptor = source_file(&root, b"data");
        let admission = begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap();
        let StartAdmission::Intent(record) = admission else {
            panic!("expected a fresh intent");
        };
        let record = *record;
        assert_eq!(record.phase, Phase::Intent);
        assert_eq!(registry.active, Some(1));
        assert_eq!(registry.watermark, 1);

        let mut descriptor = descriptor;
        let sealed = seal_source(&config, &root, &record, &mut descriptor);
        let copied = sealed.unwrap();
        assert_ne!(copied.sha256, record.source.sha256);
        let error =
            finalize_start(&mut registry, &config, &root, &request, Ok(copied)).unwrap_err();
        assert_eq!(error, ErrorCode::InvalidRequest);
        let settled = &registry.records[&1];
        assert_eq!(settled.state, State::Settled);
        assert_eq!(settled.outcome.as_deref(), Some("refused-source-mismatch"));
        assert_eq!(settled.plan, None);
        assert_eq!(registry.active, None);
        assert!(!settled.workspace(&root).exists());

        // A descriptor whose regular-file facts do not match the declared
        // size is refused by the bounded metadata check before any intent.
        let mut registry = empty_registry(&config.instance);
        let request = with_canonical_digests(start_request(&config, |request| {
            if let photo::Request::Start { source, .. } = request {
                source.size = 8;
                source.sha256 = format!("{:x}", Sha256::digest(b"data"));
            }
        }));
        let descriptor = source_file(&root, b"data");
        assert_eq!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::InvalidRequest
        );
        assert!(registry.records.is_empty());
        assert_eq!(registry.active, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn verified_source_seals_and_derives_the_one_admitted_plan() {
        let root = temp_dir("planned");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let payload = b"raw-bytes";
        let request = with_canonical_digests(start_request(&config, |request| {
            if let photo::Request::Start { source, .. } = request {
                source.size = payload.len() as u64;
                source.sha256 = format!("{:x}", Sha256::digest(payload));
            }
        }));
        let descriptor = source_file(&root, payload);
        let StartAdmission::Intent(record) = begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap() else {
            panic!("expected a fresh intent");
        };
        let mut descriptor = descriptor;
        let copied = seal_source(&config, &root, &record, &mut descriptor).unwrap();
        assert_eq!(copied.sha256, record.source.sha256);
        // The sealed snapshot carries the qualified container extension so
        // the pinned engine selects the qualified decoder, and it is read-only
        // inside a directory the unprivileged worker can traverse.
        let sealed_path = record.workspace(&root).join("source").join("source.ARW");
        let metadata = fs::metadata(&sealed_path).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o444);
        assert_eq!(fs::read(&sealed_path).unwrap(), payload);
        assert_eq!(
            fs::metadata(record.workspace(&root).join("source"))
                .unwrap()
                .mode()
                & 0o777,
            0o755
        );
        let body = finalize_start(&mut registry, &config, &root, &request, Ok(copied)).unwrap();
        let ResultBody::Receipt { receipt } = body else {
            panic!("expected a receipt");
        };
        assert_eq!(receipt.state, "accepted");
        assert_eq!(receipt.outcome, None);
        assert_eq!(receipt.sequence, 1);
        let planned = &registry.records[&1];
        assert_eq!(planned.phase, Phase::Planned);
        assert_eq!(
            planned.plan.as_ref().map(|plan| plan.steps.as_slice()),
            Some(&["develop".to_string()][..])
        );
        // Provisioning creates the same attempt workspace the seal already
        // made. The two steps must compose: a second creation that failed
        // would settle every attempt as interrupted before the engine
        // container is ever created.
        backend::create_private_directory(&record.workspace(&root)).unwrap();
        assert_eq!(
            fs::metadata(record.workspace(&root)).unwrap().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unsupported_profiles_sizes_and_digests_are_refused_before_the_descriptor() {
        let root = temp_dir("refusals");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let refuse = |registry: &mut Registry, request: photo::Request| {
            begin_start(
                registry,
                &config,
                &root,
                &request,
                None,
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err()
        };
        let canonical = |mut request: photo::Request| {
            if let photo::Request::Start {
                ref source,
                ref recipe,
                ref policy,
                ref bundle,
                ref mut recipe_digest,
                ref mut manifest_sha256,
                ..
            } = request
            {
                *recipe_digest = recipe_digest_of(recipe).unwrap();
                *manifest_sha256 = manifest_digest_of(source, recipe, policy, bundle).unwrap();
            }
            request
        };

        // An unqualified source class has no admitted plan.
        let foreign = canonical(start_request(&config, |request| {
            if let photo::Request::Start { source, .. } = request {
                source.profile_id = "canon-eos-r5".into();
            }
        }));
        assert_eq!(refuse(&mut registry, foreign), ErrorCode::InvalidRequest);

        // A declared size beyond the configured ceiling is refused even
        // though the closed envelope bound would allow it.
        let oversized = canonical(start_request(&config, |request| {
            if let photo::Request::Start { source, .. } = request {
                source.size = 8192;
            }
        }));
        assert_eq!(refuse(&mut registry, oversized), ErrorCode::InvalidRequest);

        // Exposure outside the qualified bundle range is refused.
        let over_range = canonical(start_request(&config, |request| {
            if let photo::Request::Start { recipe, .. } = request {
                recipe.exposure_milli_ev = 1001;
            }
        }));
        assert_eq!(refuse(&mut registry, over_range), ErrorCode::InvalidRequest);

        // A forged recipe or manifest digest fails closed.
        let forged = start_request(&config, |request| {
            if let photo::Request::Start { recipe_digest, .. } = request {
                *recipe_digest = "e".repeat(64);
            }
        });
        assert_eq!(refuse(&mut registry, forged), ErrorCode::InvalidRequest);
        let forged_manifest = start_request(&config, |request| {
            if let photo::Request::Start {
                manifest_sha256, ..
            } = request
            {
                *manifest_sha256 = "f".repeat(64);
            }
        });
        assert_eq!(
            refuse(&mut registry, forged_manifest),
            ErrorCode::InvalidRequest
        );

        // A different policy or bundle identity is incompatible.
        let foreign_policy = canonical(start_request(&config, |request| {
            if let photo::Request::Start { policy, .. } = request {
                *policy = "9".repeat(64);
            }
        }));
        assert_eq!(
            refuse(&mut registry, foreign_policy),
            ErrorCode::IncompatiblePolicy
        );
        let foreign_bundle = canonical(start_request(&config, |request| {
            if let photo::Request::Start { bundle, .. } = request {
                *bundle = "8".repeat(64);
            }
        }));
        assert_eq!(
            refuse(&mut registry, foreign_bundle),
            ErrorCode::IncompatibleBundle
        );

        // Sequence discipline and a missing descriptor.
        assert_eq!(
            refuse(&mut registry, canonical(start_request(&config, |_| {}))),
            ErrorCode::InvalidRequest
        );
        let skipped = canonical(start_request(&config, |request| {
            if let photo::Request::Start { sequence, .. } = request {
                *sequence = 2;
            }
        }));
        assert_eq!(refuse(&mut registry, skipped), ErrorCode::UnknownAttempt);

        assert!(registry.records.is_empty());
        assert_eq!(registry.active, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reserved_storage_and_inode_bounds_gate_admission() {
        let root = temp_dir("budget");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        // One long-running attempt already reserves the whole staged budget.
        let mut hog = record_for(1, Phase::Planned);
        hog.source.size = config.staged_storage_bytes_max;
        registry.records.insert(1, hog);
        registry.active = Some(1);
        registry.watermark = 1;
        let request = with_canonical_digests(start_request(&config, |request| {
            if let photo::Request::Start { sequence, .. } = request {
                *sequence = 2;
            }
        }));
        // The byte ceiling gates admission before anything is staged.
        let descriptor = source_file(&root, b"data");
        assert_eq!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::ResourceBudget
        );
        // Below the byte ceiling, the fixed per-attempt inode shape still
        // bounds admission.
        assert_eq!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::ResourceBudget
        );
        registry.records.get_mut(&1).unwrap().source.size = 0;
        let mut tight = config.clone();
        tight.staged_storage_inodes_max = INODES_PER_ATTEMPT - 1;
        assert_eq!(
            begin_start(
                &mut registry,
                &tight,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::ResourceBudget
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn output_and_validation_are_refused_before_a_validated_transfer() {
        let root = temp_dir("early");
        let mut registry = empty_registry("0".repeat(32).as_str());
        registry.records.insert(1, record_for(1, Phase::Planned));
        registry.active = Some(1);
        registry.watermark = 1;
        registry.records.insert(2, {
            let mut record = record_for(2, Phase::OutputReady);
            record.output = Some(OutputIdentity {
                size: 10,
                sha256: format!("{:x}", Sha256::digest(b"tiff-bytes")),
                width: 4,
                height: 3,
            });
            record
        });
        registry.active = Some(2);
        registry.watermark = 2;
        let executor = executor_with(&root, registry);
        let incarnation = "a".repeat(32);

        // Unknown attempt identities are refused.
        assert_eq!(
            executor.inspect(&incarnation, "export-1", 9).unwrap_err(),
            ErrorCode::UnknownAttempt
        );
        assert_eq!(
            executor.inspect(&incarnation, "export-1", 0).unwrap_err(),
            ErrorCode::Expired
        );
        assert_eq!(
            executor
                .output(&incarnation, "export-1", 9, None)
                .unwrap_err(),
            ErrorCode::UnknownAttempt
        );
        // A planned attempt has no validated output to transfer.
        assert_eq!(
            executor
                .output(&incarnation, "export-1", 1, None)
                .unwrap_err(),
            ErrorCode::InvalidRequest
        );
        // Validation before any transfer is refused.
        assert_eq!(
            executor
                .validate_output(&incarnation, "export-1", 2, 10, &"4".repeat(64), true)
                .unwrap_err(),
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            executor
                .validate_output(&"b".repeat(32), "export-1", 2, 10, &"4".repeat(64), true)
                .unwrap_err(),
            ErrorCode::StaleIncarnation
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn output_transfer_returns_the_bounded_receipt_exactly_once() {
        let root = temp_dir("transfer");
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::OutputReady);
        let payload = b"tiff-bytes";
        record.output = Some(OutputIdentity {
            size: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
            width: 4,
            height: 3,
        });
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        let executor = executor_with(&root, registry);
        let incarnation = "a".repeat(32);

        // The launcher-owned result file at its attempt path.
        let result_path = root
            .join("attempts")
            .join("b".repeat(32))
            .join("work")
            .join("output");
        fs::create_dir_all(&result_path).unwrap();
        fs::write(result_path.join("development.tif"), payload).unwrap();

        // The service output descriptor: writable, empty, zero offset.
        let output_path = root.join("service-output");
        let descriptor = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(&output_path)
            .unwrap();

        let body = executor
            .output(&incarnation, "export-1", 1, Some(descriptor))
            .unwrap();
        let ResultBody::Output { receipt } = body else {
            panic!("expected an output receipt");
        };
        assert_eq!(receipt.export_id, "export-1");
        assert_eq!(receipt.incarnation, incarnation);
        assert_eq!(receipt.sequence, 1);
        assert_eq!(receipt.target, crate::protocol::PHOTO_WORKLOAD);
        assert_eq!(receipt.size, payload.len() as u64);
        assert_eq!(receipt.sha256, format!("{:x}", Sha256::digest(payload)));
        assert_eq!(fs::read(&output_path).unwrap(), payload);
        // The transfer is durably recorded for the validation acknowledgement.
        assert!(executor.record(1).unwrap().output_transferred);
        // A repeated transfer of the same attempt is refused with the closed
        // conflict outcome: the durable claim admits exactly one service
        // artifact, and the refused descriptor never gains bytes.
        let descriptor = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(root.join("service-output-2"))
            .unwrap();
        assert_eq!(
            executor
                .output(&incarnation, "export-1", 1, Some(descriptor))
                .unwrap_err(),
            ErrorCode::Conflict
        );
        assert_eq!(
            fs::read(root.join("service-output-2")).unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(fs::read(&output_path).unwrap(), payload);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn output_side_operations_are_bound_to_the_durable_export_identity() {
        let root = temp_dir("export-bound");
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::OutputReady);
        record.state = State::Settling;
        let payload = b"tiff-bytes";
        record.output = Some(OutputIdentity {
            size: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
            width: 4,
            height: 3,
        });
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        let executor = executor_with(&root, registry);
        let incarnation = "a".repeat(32);

        // The launcher-owned result file at its attempt path.
        let result_path = root
            .join("attempts")
            .join("b".repeat(32))
            .join("work")
            .join("output");
        fs::create_dir_all(&result_path).unwrap();
        fs::write(result_path.join("development.tif"), payload).unwrap();

        // Every output-side operation under a foreign export identity is
        // refused with the closed conflict outcome and changes nothing.
        let foreign = "export-2";
        assert_eq!(
            executor.inspect(&incarnation, foreign, 1).unwrap_err(),
            ErrorCode::Conflict
        );
        assert_eq!(
            executor
                .validate_output(&incarnation, foreign, 1, 10, &"4".repeat(64), true)
                .unwrap_err(),
            ErrorCode::Conflict
        );
        assert_eq!(
            executor.cancel(&incarnation, foreign, 1).unwrap_err(),
            ErrorCode::Conflict
        );
        let refused_descriptor = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(root.join("service-output-refused"))
            .unwrap();
        assert_eq!(
            executor
                .output(&incarnation, foreign, 1, Some(refused_descriptor))
                .unwrap_err(),
            ErrorCode::Conflict
        );
        let record = executor.record(1).unwrap();
        assert!(!record.output_transferred);
        assert_eq!(record.validation_ack, None);
        assert!(!record.cancellation_requested);
        assert_eq!(record.outcome, None);
        assert_eq!(record.state, State::Settling);
        assert_eq!(
            fs::read(root.join("service-output-refused")).unwrap(),
            Vec::<u8>::new()
        );

        // The matching export identity still drives the whole flow: inspect,
        // the one transfer, the validation acknowledgement, and cancellation.
        let ResultBody::Receipt { receipt } =
            executor.inspect(&incarnation, "export-1", 1).unwrap()
        else {
            panic!("expected a receipt");
        };
        assert_eq!(receipt.export_id, "export-1");
        let descriptor = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(root.join("service-output"))
            .unwrap();
        assert!(
            executor
                .output(&incarnation, "export-1", 1, Some(descriptor))
                .is_ok()
        );
        assert_eq!(fs::read(root.join("service-output")).unwrap(), payload);
        assert!(
            executor
                .validate_output(
                    &incarnation,
                    "export-1",
                    1,
                    payload.len() as u64,
                    &format!("{:x}", Sha256::digest(payload)),
                    true
                )
                .is_ok()
        );
        assert_eq!(executor.record(1).unwrap().validation_ack, Some(true));
        assert!(executor.cancel(&incarnation, "export-1", 1).is_ok());
        assert!(executor.record(1).unwrap().cancellation_requested);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn admission_measures_the_configured_reserve_and_ancestor_headroom() {
        let root = temp_dir("headroom");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let request = with_canonical_digests(start_request(&config, |_| {}));
        let descriptor = source_file(&root, b"data");
        let begin = |registry: &mut Registry,
                     config: &Config,
                     headroom: &Headroom|
         -> Result<StartAdmission, ErrorCode> {
            begin_start(
                registry,
                config,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                headroom,
            )
        };

        // Control-path storage below the configured reserve refuses
        // admission and never persists a start intent.
        let starved = Headroom {
            control_free_bytes: config.control_reserve_bytes - 1,
            ancestor_headroom_bytes: u64::MAX,
        };
        assert_eq!(
            begin(&mut registry, &config, &starved).unwrap_err(),
            ErrorCode::Unavailable
        );
        assert!(registry.records.is_empty());
        assert_eq!(registry.active, None);
        // Shared-ancestor headroom below the configured allowance is the
        // same closed refusal.
        let starved = Headroom {
            control_free_bytes: u64::MAX,
            ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes - 1,
        };
        assert_eq!(
            begin(&mut registry, &config, &starved).unwrap_err(),
            ErrorCode::Unavailable
        );
        assert!(registry.records.is_empty());
        assert_eq!(registry.active, None);

        // Measured values at or above both configured boundaries admit.
        let StartAdmission::Intent(record) =
            begin(&mut registry, &config, &satisfied_headroom()).unwrap()
        else {
            panic!("expected a fresh intent");
        };
        assert_eq!(record.phase, Phase::Intent);
        assert_eq!(registry.active, Some(1));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn headroom_boundaries_are_the_exact_configured_comparisons() {
        let config = test_config(Path::new("/tmp"));
        let met = Headroom {
            control_free_bytes: config.control_reserve_bytes,
            ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes,
        };
        assert!(met.satisfied(&config).is_ok());
        let unmet = Headroom {
            control_free_bytes: config.control_reserve_bytes,
            ancestor_headroom_bytes: config.shared_ancestor_headroom_bytes - 1,
        };
        assert_eq!(
            unmet.satisfied(&config).unwrap_err(),
            ErrorCode::Unavailable
        );
        // The live measurements are real byte quantities the admission path
        // can compare: the control filesystem reports free space and the
        // kernel reports available memory.
        assert!(control_free_bytes(Path::new("/tmp")).unwrap() > 0);
        assert!(meminfo_available_bytes().unwrap() > 0);
        if let Ok(headroom) = shared_ancestor_headroom() {
            assert!(headroom <= meminfo_available_bytes().unwrap());
        }
    }

    #[test]
    fn replay_binds_the_declared_source_profile_identity() {
        let root = temp_dir("replay-profile");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let request = with_canonical_digests(start_request(&config, |_| {}));
        let descriptor = source_file(&root, b"data");
        let StartAdmission::Intent(_) = begin_start(
            &mut registry,
            &config,
            &root,
            &request,
            Some(&descriptor),
            &format!("sha256:{}", "1".repeat(64)),
            true,
            &satisfied_headroom(),
        )
        .unwrap() else {
            panic!("expected a fresh intent");
        };

        // The unchanged tuple, including its profile, replays.
        assert!(matches!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &request,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap(),
            StartAdmission::Replay(_)
        ));

        // A different profile under the same attempt identity and the same
        // declared digest is a conflict, never a replay: the durable digest
        // binds the profile that was admitted.
        let mut forged = request.clone();
        if let photo::Request::Start { source, .. } = &mut forged {
            source.profile_id = "sony-ilce-7cm2-arw".into();
        }
        assert_eq!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &forged,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::Conflict
        );

        // An honestly recomputed digest over the changed profile conflicts
        // as well.
        let foreign = with_canonical_digests(forged);
        assert_eq!(
            begin_start(
                &mut registry,
                &config,
                &root,
                &foreign,
                Some(&descriptor),
                &format!("sha256:{}", "1".repeat(64)),
                true,
                &satisfied_headroom(),
            )
            .unwrap_err(),
            ErrorCode::Conflict
        );
        assert_eq!(registry.records.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn validate_acknowledgement_is_accounted_exactly_once() {
        let root = temp_dir("ack");
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::OutputReady);
        record.state = State::Settling;
        record.output = Some(OutputIdentity {
            size: 10,
            sha256: "4".repeat(64),
            width: 4,
            height: 3,
        });
        record.output_transferred = true;
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        let executor = executor_with(&root, registry);
        let incarnation = "a".repeat(32);

        let body = executor
            .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), true)
            .unwrap();
        let ResultBody::Receipt { receipt } = body else {
            panic!("expected a receipt");
        };
        assert_eq!(receipt.state, "settling");
        assert_eq!(executor.record(1).unwrap().validation_ack, Some(true));

        // Replaying the same acknowledgement resolves to the same receipt.
        assert!(
            executor
                .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), true)
                .is_ok()
        );
        // A different acknowledgement value is a conflict.
        assert_eq!(
            executor
                .validate_output(&incarnation, "export-1", 1, 10, &"4".repeat(64), false)
                .unwrap_err(),
            ErrorCode::Conflict
        );
        // A mismatched output echo never acknowledges.
        let mut conflicting = empty_registry("0".repeat(32).as_str());
        let mut other = record_for(1, Phase::OutputReady);
        other.state = State::Settling;
        other.output_transferred = true;
        other.output = Some(OutputIdentity {
            size: 12,
            sha256: "5".repeat(64),
            width: 4,
            height: 3,
        });
        conflicting.records.insert(1, other);
        conflicting.active = Some(1);
        conflicting.watermark = 1;
        let other_executor = executor_with(&root, conflicting);
        assert_eq!(
            other_executor
                .validate_output(&incarnation, "export-1", 1, 10, &"5".repeat(64), true)
                .unwrap_err(),
            ErrorCode::Conflict
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancellation_and_completion_settle_exactly_once() {
        let root = temp_dir("settle");
        let registry = {
            let mut registry = empty_registry("0".repeat(32).as_str());
            let mut record = record_for(1, Phase::Released);
            record.state = State::Running;
            registry.records.insert(1, record);
            registry.active = Some(1);
            registry.watermark = 1;
            registry
        };
        let executor = executor_with(&root, registry);
        let first = {
            let executor = executor.clone();
            thread::spawn(move || {
                let _ = executor.settle(1, Outcome::Cancelled);
            })
        };
        let second = {
            let executor = executor.clone();
            thread::spawn(move || {
                let _ = executor.settle(1, Outcome::Deadline);
            })
        };
        first.join().unwrap();
        second.join().unwrap();
        // Exactly one terminal outcome wins; the loser never overwrites it.
        let settled = executor.record(1).unwrap();
        assert!(matches!(
            settled.outcome.as_deref(),
            Some("cancelled") | Some("deadline")
        ));
        let body = executor.settle(1, Outcome::Completed).unwrap();
        let ResultBody::Receipt { receipt } = body else {
            panic!("expected a receipt");
        };
        assert_eq!(receipt.outcome, settled.outcome);
        // A late Cancel request for a terminal attempt never re-settles.
        let body = executor.cancel(&"a".repeat(32), "export-1", 1).unwrap();
        let ResultBody::Receipt { receipt } = body else {
            panic!("expected a receipt");
        };
        assert_eq!(receipt.outcome, settled.outcome);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn durable_snapshot_survives_restart_with_sequence_and_active_state() {
        let root = temp_dir("restart");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let mut settled = record_for(1, Phase::OutputReady);
        settled.state = State::Settled;
        settled.outcome = Some("completed".into());
        settled.cleanup = Cleanup::Complete;
        settled.settled_at_unix_ms = Some(1);
        settled.output = Some(OutputIdentity {
            size: 10,
            sha256: "4".repeat(64),
            width: 4,
            height: 3,
        });
        let mut active = record_for(2, Phase::Released);
        active.state = State::Running;
        registry.records.insert(1, settled);
        registry.records.insert(2, active);
        registry.active = Some(2);
        registry.watermark = 2;
        registry.parent_identity = Some(ParentIdentity {
            invocation: "c".repeat(32),
            inode: 2,
        });
        persist(&root, &registry).unwrap();

        // The restarted launcher loads the same journal.
        let loaded = load(&root).unwrap().unwrap();
        validate_registry(&loaded, &config).unwrap();
        assert_eq!(loaded.incarnation, registry.incarnation);
        assert_eq!(loaded.watermark, 2);
        assert_eq!(loaded.active, Some(2));
        let capability = capability_body(&loaded, &config, true);
        let ResultBody::Capability {
            next_sequence,
            active: _,
            availability,
            ..
        } = capability
        else {
            panic!("expected a capability");
        };
        assert_eq!(next_sequence, 3);
        assert_eq!(availability, Availability::Available);
        let ResultBody::Capability {
            active: receipt, ..
        } = capability_body(&loaded, &config, true)
        else {
            panic!("expected a capability");
        };
        let receipt = receipt.expect("an active receipt");
        assert_eq!(receipt.sequence, 2);
        assert_eq!(receipt.state, "running");

        // A tampered snapshot is refused, never partially recovered.
        let mut tampered = load(&root).unwrap().unwrap();
        tampered.records.get_mut(&2).unwrap().outcome = Some("completed".into());
        assert_eq!(
            validate_registry(&tampered, &config).unwrap_err(),
            ErrorCode::Uncertain
        );
        let mut fabricated = load(&root).unwrap().unwrap();
        fabricated.records.get_mut(&1).unwrap().cleanup = Cleanup::Pending;
        assert_eq!(
            validate_registry(&fabricated, &config).unwrap_err(),
            ErrorCode::Uncertain
        );
        // An unsettled record never claims a cleanup it cannot have run.
        let mut premature = load(&root).unwrap().unwrap();
        premature.records.get_mut(&2).unwrap().cleanup = Cleanup::Complete;
        assert_eq!(
            validate_registry(&premature, &config).unwrap_err(),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_interrupted_settlement_loads_so_reconcile_can_finish_it() {
        let root = temp_dir("settling-recovery");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        // A settlement records the terminal outcome and persists it before the
        // attempt boundary is removed. A launcher killed in that window leaves
        // exactly this state, and `reconcile` retries the pending cleanup, so
        // it must load instead of refusing every later start.
        let mut settling = record_for(1, Phase::Released);
        settling.state = State::Settling;
        settling.outcome = Some("interrupted".into());
        registry.records.insert(1, settling);
        registry.active = Some(1);
        registry.watermark = 1;
        registry.parent_identity = Some(ParentIdentity {
            invocation: "c".repeat(32),
            inode: 2,
        });
        persist(&root, &registry).unwrap();

        let loaded = load(&root).unwrap().unwrap();
        validate_registry(&loaded, &config).unwrap();
        let pending = loaded.records.get(&1).unwrap();
        assert_eq!(pending.cleanup, Cleanup::Pending);
        assert!(pending.outcome.is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discovery_resolves_a_pending_manager_phase_from_the_observed_attempt() {
        let root = temp_dir("discover-phase");
        // A worker that exits before its release-gate pause leaves that pause
        // recorded: `docker pause` cannot succeed on a worker that already
        // exited. The recorded slice, mount and container identities are what
        // the launcher owns, so reconciliation resolves the marker here instead
        // of blocking settlement and cleanup forever.
        for phase in [
            ManagerPhase::Slice,
            ManagerPhase::Mount,
            ManagerPhase::Start,
            ManagerPhase::Pause,
            ManagerPhase::Unpause,
        ] {
            let mut registry = empty_registry("0".repeat(32).as_str());
            let mut record = record_for(1, Phase::Released);
            record.state = State::Settling;
            record.outcome = Some("interrupted".into());
            record.manager_pending = Some(phase);
            registry.records.insert(1, record);
            registry.active = Some(1);
            registry.watermark = 1;
            registry.parent_identity = Some(ParentIdentity {
                invocation: "c".repeat(32),
                inode: 2,
            });
            let executor = executor_with(&root, registry);
            let mut record = executor.record(1).unwrap();
            executor.discover(&mut record).unwrap();
            assert_eq!(record.manager_pending, None, "{phase:?}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discovery_keeps_a_pending_slice_stop_ambiguous() {
        let root = temp_dir("discover-stop");
        let mut registry = empty_registry("0".repeat(32).as_str());
        let mut record = record_for(1, Phase::Released);
        record.state = State::Settling;
        record.outcome = Some("interrupted".into());
        record.manager_pending = Some(ManagerPhase::SliceStop);
        registry.records.insert(1, record);
        registry.active = Some(1);
        registry.watermark = 1;
        registry.parent_identity = Some(ParentIdentity {
            invocation: "c".repeat(32),
            inode: 2,
        });
        let executor = executor_with(&root, registry);
        let mut record = executor.record(1).unwrap();
        // Whether the attempt boundary still exists decides what cleanup may
        // remove, and only a confirmed stop return clears this phase.
        assert_eq!(
            executor.discover(&mut record).unwrap_err(),
            ErrorCode::Uncertain
        );
        assert_eq!(record.manager_pending, Some(ManagerPhase::SliceStop));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cleanup_refuses_a_record_whose_manager_phase_is_still_pending() {
        let root = temp_dir("cleanup-phase");
        let registry = empty_registry("0".repeat(32).as_str());
        let executor = executor_with(&root, registry);
        let mut record = record_for(1, Phase::Released);
        record.manager_pending = Some(ManagerPhase::Pause);
        // An unresolved manager effect is never torn down on an assumption.
        assert_eq!(
            executor.cleanup(&mut record).unwrap_err(),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_registry_initializes_a_fresh_incarnation() {
        let root = temp_dir("fresh-registry");
        let config = test_config(&root);
        // A root with no registry restores nothing and initializes a fresh
        // registry, exactly as a first start.
        let registry = restore_registry(&root, &config).unwrap();
        validate_registry(&registry, &config).unwrap();
        assert_eq!(registry.version, 1);
        assert_eq!(registry.instance, config.instance);
        assert!(registry.records.is_empty());
        assert!(!registry.parent_pending);
        assert_eq!(registry.active, None);
        // Each uninitialized root gets its own incarnation.
        let again = restore_registry(&root, &config).unwrap();
        assert_ne!(again.incarnation, registry.incarnation);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_open_removes_only_the_claim_this_process_created() {
        let root = temp_dir("fresh-claim");
        let path = root.join("instance.claim");
        let claim_root = root.display().to_string();
        // The acquisition itself arms the lease: a fresh claim is removed
        // when the open fails after claiming.
        let claim = journal::hold_claim(&path, &root, &claim_root).unwrap();
        drop(claim);
        assert!(!path.try_exists().unwrap());
        // Disarming by value keeps the claim for the executor's lifetime.
        drop(
            journal::hold_claim(&path, &root, &claim_root)
                .unwrap()
                .take(),
        );
        assert!(path.try_exists().unwrap());
        // A claim created by an earlier owner is never a lease, so a failure
        // here can never remove it; the quarantine in the shared claim path
        // is what refuses a registry-less one.
        fs::File::create(root.join("registry.json")).unwrap();
        // Dropping the non-lease keeps the file: a failure here can never
        // remove a claim created by an earlier owner.
        drop(journal::hold_claim(&path, &root, &claim_root).unwrap());
        assert!(path.try_exists().unwrap());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn expired_settled_receipts_leave_retention_while_incomplete_cleanup_stays() {
        let root = temp_dir("expire");
        let config = test_config(&root);
        let mut registry = empty_registry(&config.instance);
        let mut settled = record_for(1, Phase::Intent);
        settled.state = State::Settled;
        settled.outcome = Some("refused-source-mismatch".into());
        settled.cleanup = Cleanup::Complete;
        settled.settled_at_unix_ms = Some(1000);
        let mut stuck = record_for(2, Phase::Released);
        stuck.state = State::Settling;
        stuck.outcome = Some("interrupted".into());
        stuck.cleanup = Cleanup::Uncertain;
        stuck.settled_at_unix_ms = Some(1000);
        registry.records.insert(1, settled);
        registry.records.insert(2, stuck);
        registry.active = Some(2);
        registry.watermark = 2;
        // Well inside the retention window both stay.
        expire(&mut registry, 2000, 86_400).unwrap();
        assert_eq!(registry.records.len(), 2);
        // After retention the complete receipt expires, the uncertain one stays.
        expire(&mut registry, 1000 + 86_400 * 1000, 86_400).unwrap();
        assert!(registry.records.contains_key(&2));
        assert!(!registry.records.contains_key(&1));
        assert_eq!(registry.active, Some(2));
        fs::remove_dir_all(&root).unwrap();
    }
}
