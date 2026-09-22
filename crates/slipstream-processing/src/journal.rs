use crate::{
    backend::{Backend, secure_directory},
    protocol::*,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManagerPhase {
    Slice,
    Mount,
    Create,
    CreateReturned,
    Start,
    Pause,
    Unpause,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub receipt: Receipt,
    pub launch_id: String,
    pub image_id: String,
    pub unit_invocation: Option<String>,
    pub cgroup_inode: Option<u64>,
    pub mount_id: Option<u64>,
    pub released: bool,
    pub termination_reason: Option<Outcome>,
    pub manager_pending: Option<ManagerPhase>,
    pub settled_at_unix_ms: Option<u64>,
}

impl Record {
    pub fn unit(&self) -> &str {
        &self
            .receipt
            .runtime
            .as_ref()
            .expect("record runtime")
            .attempt_unit
    }
    pub fn container_id(&self) -> Result<&str, ErrorCode> {
        self.receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_deref())
            .ok_or(ErrorCode::Uncertain)
    }
    pub fn container_name(&self) -> String {
        format!("slipstream-processing-{}", self.launch_id)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParentIdentity {
    pub invocation: String,
    pub inode: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u8,
    instance: String,
    incarnation: String,
    watermark: u64,
    parent_pending: bool,
    parent_identity: Option<ParentIdentity>,
    active: Option<u64>,
    records: BTreeMap<u64, Record>,
}

struct Data {
    registry: Registry,
    available: bool,
}

/// One exclusive operational owner. It does not own the Photo Library or Export state.
pub struct Executor {
    config: Config,
    backend: Backend,
    data: Mutex<Data>,
    image_id: String,
    bundle: String,
    _lock: File,
    _instance_claim: File,
}

impl Executor {
    pub fn open(config: Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            return Err(ErrorCode::Unauthorized);
        }
        let root = Path::new(&config.root);
        secure_directory(root, 0)?;
        secure_directory(
            Path::new(&config.socket)
                .parent()
                .ok_or(ErrorCode::Unavailable)?,
            0,
        )?;
        let instance_claim = claim_instance(&config)?;
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
        for name in ["attempts", "docker-client", "faults"] {
            let path = root.join(name);
            prepare_private_directory(&path)?;
        }
        let backend = Backend {
            config: config.clone(),
        };
        let image_id = backend.image()?;
        let bundle = digest(&serde_json::to_vec(&(PROFILE, &image_id)).expect("bundle identity"));
        let registry = load(root)?.unwrap_or(Registry {
            version: 1,
            instance: config.instance.clone(),
            incarnation: random_id()?,
            watermark: 0,
            parent_pending: false,
            parent_identity: None,
            active: None,
            records: BTreeMap::new(),
        });
        validate_registry(&registry, &config)?;
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
        persist(root, &registry)?;
        let executor = Arc::new(Self {
            config,
            backend,
            data: Mutex::new(Data {
                registry,
                available: false,
            }),
            image_id,
            bundle,
            _lock: lock,
            _instance_claim: instance_claim,
        });
        Ok(executor)
    }

    pub(crate) fn recover_async(self: &Arc<Self>) {
        let owner = self.clone();
        thread::spawn(move || {
            if owner.recover().is_err() {
                owner.block_active();
            }
        });
    }

    pub(crate) fn handle(
        self: &Arc<Self>,
        request: Request,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        if request.instance() != self.config.instance {
            return Err(ErrorCode::WrongInstance);
        }
        self.backend.check_caller(peer_pid)?;
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        match request {
            Request::Reconcile { .. } => Ok(ResultBody::Capability {
                capability: "qualification-only".into(),
                instance: self.config.instance.clone(),
                incarnation: data.registry.incarnation.clone(),
                next_sequence: data
                    .registry
                    .watermark
                    .checked_add(1)
                    .ok_or(ErrorCode::Capacity)?,
                policy: self.config.policy(),
                bundle: self.bundle.clone(),
                availability: if data.available
                    && self
                        .backend
                        .admission_ready(data.registry.parent_identity.as_ref())
                        .is_ok()
                {
                    Availability::Available
                } else {
                    Availability::Blocked
                },
                active: data.registry.active.and_then(|sequence| {
                    data.registry
                        .records
                        .get(&sequence)
                        .map(|record| record.receipt.clone())
                }),
            }),
            Request::Start {
                incarnation,
                sequence,
                policy,
                bundle,
                workload,
                ..
            } => {
                if incarnation != data.registry.incarnation {
                    return Err(ErrorCode::StaleIncarnation);
                }
                expire(
                    &mut data.registry,
                    now()?,
                    self.config.receipt_retention_seconds,
                )?;
                if let Some(record) = data.registry.records.get(&sequence) {
                    if record.receipt.policy != policy
                        || record.receipt.bundle != bundle
                        || record.receipt.workload != workload
                    {
                        return Err(ErrorCode::Conflict);
                    }
                    return Ok(ResultBody::Receipt {
                        receipt: record.receipt.clone(),
                    });
                }
                if sequence <= data.registry.watermark {
                    return Err(ErrorCode::Expired);
                }
                if sequence
                    != data
                        .registry
                        .watermark
                        .checked_add(1)
                        .ok_or(ErrorCode::Capacity)?
                {
                    return Err(ErrorCode::UnknownAttempt);
                }
                if !data.available {
                    return Err(ErrorCode::Unavailable);
                }
                if data.registry.active.is_some() {
                    return Err(ErrorCode::Busy);
                }
                if data.registry.records.len() >= 256 {
                    return Err(ErrorCode::Capacity);
                }
                if policy != self.config.policy() {
                    return Err(ErrorCode::IncompatiblePolicy);
                }
                if bundle != self.bundle {
                    return Err(ErrorCode::IncompatibleBundle);
                }
                self.backend
                    .admission_ready(data.registry.parent_identity.as_ref())?;
                let time = now()?;
                let launch_id = random_id()?;
                let record = Record {
                    receipt: Receipt {
                        incarnation,
                        sequence,
                        workload,
                        policy,
                        bundle,
                        state: State::Accepted,
                        cancellation_requested: false,
                        accepted_at_unix_ms: time,
                        deadline_unix_ms: time.checked_add(30000).ok_or(ErrorCode::Capacity)?,
                        outcome: None,
                        runtime: Some(Runtime {
                            launch_id: launch_id.clone(),
                            container_id: None,
                            attempt_unit: format!(
                                "slipstreamprocessing{}-{}.slice",
                                self.config.instance, launch_id
                            ),
                        }),
                        limits: Limits::new(self.config.memory_bytes),
                        evidence: None,
                        cleanup: Cleanup::Pending,
                    },
                    launch_id,
                    image_id: self.image_id.clone(),
                    unit_invocation: None,
                    cgroup_inode: None,
                    mount_id: None,
                    released: false,
                    termination_reason: None,
                    manager_pending: None,
                    settled_at_unix_ms: None,
                };
                let mut next = data.registry.clone();
                next.active = Some(sequence);
                next.watermark = sequence;
                next.records.insert(sequence, record.clone());
                persist(Path::new(&self.config.root), &next)?;
                data.registry = next;
                let owner = self.clone();
                thread::spawn(move || {
                    if owner.execute(sequence).is_err() {
                        owner.block_active();
                    }
                });
                Ok(ResultBody::Receipt {
                    receipt: record.receipt,
                })
            }
            Request::Inspect {
                incarnation,
                sequence,
                ..
            }
            | Request::Cancel {
                incarnation,
                sequence,
                ..
            } => {
                if incarnation != data.registry.incarnation {
                    return Err(ErrorCode::StaleIncarnation);
                }
                expire(
                    &mut data.registry,
                    now()?,
                    self.config.receipt_retention_seconds,
                )?;
                let record = data.registry.records.get(&sequence).ok_or(
                    if sequence <= data.registry.watermark {
                        ErrorCode::Expired
                    } else {
                        ErrorCode::UnknownAttempt
                    },
                )?;
                Ok(ResultBody::Receipt {
                    receipt: record.receipt.clone(),
                })
            }
        }
    }

    pub(crate) fn cancel(
        self: &Arc<Self>,
        request: Request,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        let result = self.handle(request.clone(), peer_pid)?;
        let Request::Cancel { sequence, .. } = request else {
            return Ok(result);
        };
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        let mut next = data.registry.clone();
        let record = next.records.get_mut(&sequence).ok_or(ErrorCode::Expired)?;
        if record.receipt.state == State::Settled || record.receipt.outcome.is_some() {
            return Ok(ResultBody::Receipt {
                receipt: record.receipt.clone(),
            });
        }
        record.receipt.cancellation_requested = true;
        let receipt = record.receipt.clone();
        persist(Path::new(&self.config.root), &next)?;
        data.registry = next;
        Ok(ResultBody::Receipt { receipt })
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    fn record(&self, sequence: u64) -> Result<Record, ErrorCode> {
        self.data
            .lock()
            .map_err(|_| ErrorCode::Uncertain)?
            .registry
            .records
            .get(&sequence)
            .cloned()
            .ok_or(ErrorCode::Uncertain)
    }

    fn update(&self, record: &Record) -> Result<(), ErrorCode> {
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        let mut next = data.registry.clone();
        let previous = next
            .records
            .get(&record.receipt.sequence)
            .ok_or(ErrorCode::Uncertain)?;
        let mut record = record.clone();
        record.receipt.cancellation_requested |= previous.receipt.cancellation_requested;
        record.termination_reason = previous.termination_reason.or(record.termination_reason);
        if let Some(outcome) = previous.receipt.outcome
            && outcome != Outcome::Unknown
        {
            record.receipt.outcome = Some(outcome);
        }
        if record.receipt.state == State::Settled {
            if record.receipt.cleanup != Cleanup::Complete || record.receipt.outcome.is_none() {
                return Err(ErrorCode::Uncertain);
            }
            next.active = None;
        }
        next.records.insert(record.receipt.sequence, record);
        persist(Path::new(&self.config.root), &next)?;
        data.registry = next;
        Ok(())
    }

    fn block_active(&self) {
        if let Ok(mut data) = self.data.lock() {
            data.available = false;
            if let Some(sequence) = data.registry.active
                && let Some(record) = data.registry.records.get_mut(&sequence)
            {
                record.receipt.state = State::Blocked;
                record.receipt.cleanup = Cleanup::Uncertain;
            }
            let _ = persist(Path::new(&self.config.root), &data.registry);
        }
    }

    fn recover(&self) -> Result<(), ErrorCode> {
        if self
            .data
            .lock()
            .map_err(|_| ErrorCode::Uncertain)?
            .registry
            .parent_pending
        {
            return Err(ErrorCode::Uncertain);
        }
        let records: Vec<_> = self
            .data
            .lock()
            .map_err(|_| ErrorCode::Uncertain)?
            .registry
            .records
            .values()
            .cloned()
            .collect();
        let parent = self
            .data
            .lock()
            .map_err(|_| ErrorCode::Uncertain)?
            .registry
            .parent_identity
            .clone();
        self.backend.verify_parent(parent.as_ref())?;
        self.backend.scan_unowned(&records)?;
        for mut record in records {
            if record.receipt.state == State::Settled {
                continue;
            }
            self.backend.discover(&mut record)?;
            self.update(&record)?;
            let requested = if record.receipt.cancellation_requested {
                Outcome::Cancelled
            } else if now()? >= record.receipt.deadline_unix_ms {
                Outcome::Deadline
            } else {
                Outcome::Interrupted
            };
            self.settle(record, Some(requested))?;
        }
        {
            let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
            data.registry.parent_pending = true;
            persist(Path::new(&self.config.root), &data.registry)?;
        }
        let identity = self.backend.prepare_parent(parent.as_ref())?;
        {
            let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
            data.registry.parent_identity = Some(identity);
            data.registry.parent_pending = false;
            persist(Path::new(&self.config.root), &data.registry)?;
            data.available = true;
        }
        Ok(())
    }

    fn verify_parent(&self) -> Result<(), ErrorCode> {
        let identity = self
            .data
            .lock()
            .map_err(|_| ErrorCode::Uncertain)?
            .registry
            .parent_identity
            .clone()
            .ok_or(ErrorCode::Uncertain)?;
        self.backend.verify_parent(Some(&identity))
    }

    fn execute(&self, sequence: u64) -> Result<(), ErrorCode> {
        self.verify_parent()?;
        let mut record = self.record(sequence)?;
        crate::faults::at(&self.config, &record, crate::faults::Phase::Intent)?;
        let gate = self
            .backend
            .setup(&mut record, |record| self.update(record));
        let mut gate = match gate {
            Ok(gate) => gate,
            Err(_) => {
                self.backend.discover(&mut record)?;
                self.update(&record)?;
                return self.settle(record, Some(Outcome::Interrupted));
            }
        };
        let current = self.record(sequence)?;
        if current.receipt.cancellation_requested || now()? >= record.receipt.deadline_unix_ms {
            return self.settle(
                record,
                Some(if current.receipt.cancellation_requested {
                    Outcome::Cancelled
                } else {
                    Outcome::Deadline
                }),
            );
        }
        record.released = true;
        self.update(&record)?;
        crate::faults::at(&self.config, &record, crate::faults::Phase::ReleaseIntent)?;
        // Holding the instance journal mutex closes the cancel/release race.
        {
            let data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
            if data.registry.records[&sequence]
                .receipt
                .cancellation_requested
            {
                drop(data);
                return self.settle(record, Some(Outcome::Cancelled));
            }
            gate.write_all(format!("{}\n", record.launch_id).as_bytes())
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        self.backend
            .release(&mut record, |record| self.update(record))?;
        drop(gate);
        record.receipt.state = State::Running;
        self.update(&record)?;
        loop {
            let current = self.record(sequence)?;
            if !self.backend.live(&record)?.running {
                return self.settle(record, None);
            }
            if current.receipt.cancellation_requested {
                return self.settle(record, Some(Outcome::Cancelled));
            }
            if now()? >= record.receipt.deadline_unix_ms {
                return self.settle(record, Some(Outcome::Deadline));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn settle(&self, mut record: Record, requested: Option<Outcome>) -> Result<(), ErrorCode> {
        self.verify_parent()?;
        record.receipt.state = State::Settling;
        self.update(&record)?;
        if record.receipt.outcome.is_some()
            && record
                .receipt
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.populated == Some(false))
        {
            self.backend.cleanup(&record)?;
            record.receipt.cleanup = Cleanup::Complete;
            record.receipt.state = State::Settled;
            record.settled_at_unix_ms = Some(now()?);
            return self.update(&record);
        }
        let has_container = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_ref())
            .is_some();
        if has_container {
            if self.backend.live(&record)?.running {
                if record.termination_reason.is_none() {
                    record.termination_reason = Some(requested.unwrap_or(Outcome::Interrupted));
                    self.update(&record)?;
                }
                self.backend.stop(&record)?;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.backend.live(&record)?.running {
                if Instant::now() >= deadline {
                    return Err(ErrorCode::Uncertain);
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        crate::faults::at(&self.config, &record, crate::faults::Phase::Exit)?;
        if record.unit_invocation.is_some() {
            let evidence = self.backend.evidence(&record)?;
            let worker = self.backend.worker_outcome(&record)?;
            let reason =
                record
                    .termination_reason
                    .or(if !record.released { requested } else { None });
            let outcome = classify(&evidence, worker, reason);
            record.receipt.evidence = Some(evidence);
            record.receipt.outcome = Some(outcome);
        } else {
            if has_container {
                return Err(ErrorCode::Uncertain);
            }
            record.receipt.outcome = Some(requested.unwrap_or(Outcome::Interrupted));
            record.receipt.evidence = Some(Evidence {
                peak_bytes: 0,
                exit_code: None,
                docker_oom_killed: None,
                attempt_before: None,
                attempt_after: None,
                parent_before: None,
                parent_after: None,
                populated: Some(false),
            });
        }
        self.update(&record)?;
        crate::faults::at(&self.config, &record, crate::faults::Phase::Evidence)?;
        self.backend.cleanup(&record)?;
        record.receipt.cleanup = Cleanup::Complete;
        record.receipt.state = State::Settled;
        record.settled_at_unix_ms = Some(now()?);
        self.update(&record)
    }
}

pub(crate) fn classify(
    evidence: &Evidence,
    worker: Option<Outcome>,
    requested: Option<Outcome>,
) -> Outcome {
    let oom = evidence
        .attempt_before
        .as_ref()
        .zip(evidence.attempt_after.as_ref())
        .is_some_and(|(before, after)| {
            after
                .oom_kill
                .checked_sub(before.oom_kill)
                .is_some_and(|delta| delta > 0)
        });
    if oom && evidence.docker_oom_killed == Some(true) {
        return Outcome::Oom;
    }
    match (evidence.exit_code, worker) {
        (Some(0), Some(Outcome::Completed)) => return Outcome::Completed,
        (Some(20), Some(Outcome::AllocationFailed)) => return Outcome::AllocationFailed,
        (Some(21), Some(Outcome::StorageFull)) => return Outcome::StorageFull,
        (Some(76), _) => return Outcome::Deadline,
        _ => {}
    }
    if let Some(requested) = requested {
        return requested;
    }
    if evidence.exit_code.is_some() {
        Outcome::EngineFailed
    } else {
        Outcome::Unknown
    }
}

fn expire(registry: &mut Registry, time: u64, retention: u64) -> Result<(), ErrorCode> {
    let retention = retention.checked_mul(1000).ok_or(ErrorCode::Capacity)?;
    registry.records.retain(|_, record| {
        record.receipt.state != State::Settled
            || record
                .settled_at_unix_ms
                .and_then(|settled| time.checked_sub(settled))
                .is_none_or(|age| age < retention)
    });
    Ok(())
}

fn random_id() -> Result<String, ErrorCode> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| ErrorCode::Unavailable)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn claim_instance(config: &Config) -> Result<File, ErrorCode> {
    let namespace = Path::new("/var/lib/slipstream-processing/instances");
    for path in [Path::new("/var/lib/slipstream-processing"), namespace] {
        if !path.try_exists().map_err(|_| ErrorCode::Uncertain)? {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| ErrorCode::Uncertain)?;
            File::open(path.parent().ok_or(ErrorCode::Uncertain)?)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        secure_directory(path, 0)?;
    }
    let path = namespace.join(format!("{}.claim", config.instance));
    let (mut file, fresh) = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => (file, true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)
                .map_err(|_| ErrorCode::Uncertain)?,
            false,
        ),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > REQUEST_BYTES as u64
    {
        return Err(ErrorCode::Uncertain);
    }
    // SAFETY: this exact open inode is retained for the owner's entire lifetime.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ErrorCode::Busy);
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Claim {
        version: u8,
        root: String,
    }
    if fresh {
        let bytes = serde_json::to_vec(&Claim {
            version: 1,
            root: config.root.clone(),
        })
        .map_err(|_| ErrorCode::Uncertain)?;
        if bytes.len() > REQUEST_BYTES {
            return Err(ErrorCode::Uncertain);
        }
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ErrorCode::Uncertain)?;
        File::open(namespace)
            .and_then(|file| file.sync_all())
            .map_err(|_| ErrorCode::Uncertain)?;
    } else {
        let mut bytes = Vec::new();
        (&mut file)
            .take(REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ErrorCode::Uncertain)?;
        if bytes.len() > REQUEST_BYTES {
            return Err(ErrorCode::Uncertain);
        }
        let claim: Claim = serde_json::from_slice(&bytes).map_err(|_| ErrorCode::Uncertain)?;
        if claim.version != 1
            || claim.root != config.root
            || !Path::new(&config.root)
                .join("registry.json")
                .try_exists()
                .map_err(|_| ErrorCode::Uncertain)?
        {
            return Err(ErrorCode::Uncertain);
        }
    }
    Ok(file)
}

fn prepare_private_directory(path: &Path) -> Result<(), ErrorCode> {
    if !path.try_exists().map_err(|_| ErrorCode::Unavailable)? {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| ErrorCode::Unavailable)?;
    }
    secure_directory(path, 0)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ErrorCode::Unavailable)
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
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(ErrorCode::Uncertain);
    }
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(ErrorCode::Uncertain);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ErrorCode::Uncertain)
}

fn persist(root: &Path, registry: &Registry) -> Result<(), ErrorCode> {
    let bytes = serde_json::to_vec(registry).map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > 4 * 1024 * 1024 {
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
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
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

fn validate_registry(registry: &Registry, config: &Config) -> Result<(), ErrorCode> {
    if registry.version != 1
        || registry.instance != config.instance
        || !hex(&registry.incarnation, 32)
        || registry.records.len() > 257
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
            || *sequence != record.receipt.sequence
            || record.receipt.incarnation != registry.incarnation
            || record.termination_reason.is_some_and(|reason| {
                !matches!(
                    reason,
                    Outcome::Cancelled | Outcome::Deadline | Outcome::Interrupted
                )
            })
            || !hex(&record.launch_id, 32)
            || record.receipt.runtime.as_ref().is_none_or(|runtime| {
                runtime.launch_id != record.launch_id
                    || runtime.attempt_unit
                        != format!(
                            "slipstreamprocessing{}-{}.slice",
                            config.instance, record.launch_id
                        )
                    || runtime.container_id.as_ref().is_some_and(|id| !hex(id, 64))
            })
        {
            return Err(ErrorCode::Uncertain);
        }
        if record.receipt.state != State::Settled {
            if active.replace(*sequence).is_some() {
                return Err(ErrorCode::Uncertain);
            }
        } else if record.receipt.cleanup != Cleanup::Complete || record.receipt.outcome.is_none() {
            return Err(ErrorCode::Uncertain);
        }
    }
    if active != registry.active {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(kills: u64) -> Events {
        Events {
            oom: kills,
            oom_kill: kills,
            oom_group_kill: kills,
            local_oom: 0,
            local_oom_kill: 0,
            local_oom_group_kill: 0,
        }
    }
    fn evidence(code: u8, flag: bool, before: Option<u64>, after: Option<u64>) -> Evidence {
        Evidence {
            peak_bytes: 1,
            exit_code: Some(code),
            docker_oom_killed: Some(flag),
            attempt_before: before.map(events),
            attempt_after: after.map(events),
            parent_before: None,
            parent_after: None,
            populated: Some(false),
        }
    }
    #[test]
    fn foreign_symlink_directory_is_rejected_before_chmod() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "slipstream-processing-symlink-{}",
            random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join("foreign");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
        symlink(&target, root.join("attempts")).unwrap();
        assert_eq!(
            prepare_private_directory(&root.join("attempts")),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o751);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oom_requires_kernel_delta_and_runtime_evidence() {
        assert_eq!(
            classify(&evidence(137, true, Some(0), Some(1)), None, None),
            Outcome::Oom
        );
        for value in [
            evidence(137, false, Some(0), Some(1)),
            evidence(137, true, Some(1), Some(1)),
            evidence(137, true, None, Some(1)),
            evidence(137, true, Some(2), Some(1)),
            evidence(137, false, Some(0), Some(0)),
        ] {
            assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
        }
        assert_eq!(
            classify(&evidence(76, false, Some(0), Some(0)), None, None),
            Outcome::Deadline
        );
    }
    #[test]
    fn completed_result_survives_restart_and_late_cancellation() {
        let value = evidence(0, false, Some(0), Some(0));
        for requested in [None, Some(Outcome::Interrupted), Some(Outcome::Cancelled)] {
            assert_eq!(
                classify(&value, Some(Outcome::Completed), requested),
                Outcome::Completed
            );
        }
        assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
        assert_eq!(
            classify(
                &evidence(137, false, Some(0), Some(0)),
                None,
                Some(Outcome::Cancelled)
            ),
            Outcome::Cancelled
        );
        assert_eq!(
            classify(
                &evidence(137, true, Some(0), Some(1)),
                None,
                Some(Outcome::Cancelled)
            ),
            Outcome::Oom
        );
    }
    #[test]
    fn restart_preserves_all_proven_terminal_outcomes() {
        for (code, worker, expected) in [
            (
                20,
                Some(Outcome::AllocationFailed),
                Outcome::AllocationFailed,
            ),
            (21, Some(Outcome::StorageFull), Outcome::StorageFull),
            (76, None, Outcome::Deadline),
        ] {
            assert_eq!(
                classify(
                    &evidence(code, false, Some(0), Some(0)),
                    worker,
                    Some(Outcome::Interrupted)
                ),
                expected
            );
        }
    }

    fn record(sequence: u64, state: State) -> Record {
        let launch_id = format!("{sequence:032x}");
        Record {
            receipt: Receipt {
                incarnation: "1".repeat(32),
                sequence,
                workload: Workload::ProbeSuccess,
                policy: "2".repeat(64),
                bundle: "3".repeat(64),
                state,
                cancellation_requested: false,
                accepted_at_unix_ms: 0,
                deadline_unix_ms: 30000,
                outcome: Some(Outcome::Completed),
                runtime: Some(Runtime {
                    launch_id: launch_id.clone(),
                    container_id: None,
                    attempt_unit: format!(
                        "slipstreamprocessing{}-{launch_id}.slice",
                        "0".repeat(32)
                    ),
                }),
                limits: Limits::new(64 * 1024 * 1024),
                evidence: Some(evidence(0, false, Some(0), Some(0))),
                cleanup: Cleanup::Complete,
            },
            launch_id,
            image_id: format!("sha256:{}", "4".repeat(64)),
            unit_invocation: None,
            cgroup_inode: None,
            mount_id: None,
            released: true,
            termination_reason: None,
            manager_pending: None,
            settled_at_unix_ms: Some(1000),
        }
    }
    fn registry() -> Registry {
        Registry {
            version: 1,
            instance: "0".repeat(32),
            incarnation: "1".repeat(32),
            watermark: 256,
            parent_pending: false,
            parent_identity: Some(ParentIdentity {
                invocation: "a".repeat(32),
                inode: 1,
            }),
            active: None,
            records: (1..=256).map(|i| (i, record(i, State::Settled))).collect(),
        }
    }
    #[test]
    fn expiry_preserves_watermark_and_active_identity_at_capacity() {
        let mut value = registry();
        expire(&mut value, 1999, 1).unwrap();
        assert_eq!(value.records.len(), 256);
        value.active = Some(257);
        value.watermark = 257;
        value.records.insert(257, record(257, State::Running));
        expire(&mut value, 2000, 1).unwrap();
        assert_eq!(value.records.len(), 1);
        assert_eq!(value.watermark, 257);
        assert_eq!(value.active, Some(257));
        assert!(expire(&mut value, 2000, u64::MAX).is_err());
    }
    #[test]
    fn registry_rejects_duplicate_active_or_rebound_runtime() {
        let config = Config {
            version: 1,
            mode: "qualification".into(),
            instance: "0".repeat(32),
            root: "/test".into(),
            socket: "/test.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "4".repeat(64)),
            memory_bytes: 64 * 1024 * 1024,
            receipt_retention_seconds: 1,
        };
        let original = registry();
        validate_registry(&original, &config).unwrap();
        let mut changed = original.clone();
        changed
            .records
            .get_mut(&1)
            .unwrap()
            .receipt
            .runtime
            .as_mut()
            .unwrap()
            .launch_id = "9".repeat(32);
        assert!(validate_registry(&changed, &config).is_err());
        let mut changed = original.clone();
        changed.records.get_mut(&1).unwrap().receipt.state = State::Running;
        assert!(validate_registry(&changed, &config).is_err());
        changed.active = Some(1);
        validate_registry(&changed, &config).unwrap();
        changed.records.get_mut(&2).unwrap().receipt.state = State::Running;
        assert!(validate_registry(&changed, &config).is_err());
        let mut changed = original;
        changed.records.get_mut(&1).unwrap().receipt.cleanup = Cleanup::Pending;
        assert!(validate_registry(&changed, &config).is_err());
    }
}
