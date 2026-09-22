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
    SliceStop,
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
    #[serde(default)]
    pub stop_confirmed: bool,
    pub settled_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub film: Option<crate::film::Captured>,
}

impl Record {
    pub fn result_body(&self) -> ResultBody {
        if let Some(film) = &self.film {
            ResultBody::Film(Box::new(crate::film::ResultBody::Receipt {
                receipt: film.receipt(&self.receipt),
            }))
        } else {
            ResultBody::Receipt {
                receipt: self.receipt.clone(),
            }
        }
    }
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
    film: Option<(crate::film::Config, crate::film::Documents)>,
    policy: String,
    _lock: File,
    _instance_claim: File,
}

impl Executor {
    pub fn open(config: Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        Self::open_common(config, None)
    }

    pub fn open_film(config: crate::film::Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        Self::open_common(config.authority(), Some(config))
    }

    fn open_common(
        config: Config,
        film_config: Option<crate::film::Config>,
    ) -> Result<Arc<Self>, ErrorCode> {
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
        let film = film_config
            .map(|c| crate::film::Documents::load(&c).map(|d| (c, d)))
            .transpose()?;
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
        crate::faults::validate_mode(&config)?;
        let backend = Backend {
            config: config.clone(),
        };
        let image_id = backend.image()?;
        let (bundle, policy) = if let Some((film_config, documents)) = &film {
            backend.verify_film_image(&image_id, &documents.catalogue)?;
            (
                crate::film::hash(&(
                    crate::film::PROFILE,
                    &image_id,
                    &documents.catalogue.numerical_bundle,
                ))?,
                film_config.policy(&image_id)?,
            )
        } else {
            (
                digest(&serde_json::to_vec(&(PROFILE, &image_id)).expect("bundle identity")),
                config.policy(),
            )
        };
        let registry = load(root)?.unwrap_or(Registry {
            version: config.version,
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
            film,
            policy,
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
        self.handle_shared(request, peer_pid, None)
    }

    pub(crate) fn is_film(&self) -> bool {
        self.film.is_some()
    }

    pub(crate) fn handle_film(
        self: &Arc<Self>,
        request: crate::film::Request,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        use crate::film::Request as F;
        let (request, ids) = match request {
            F::Reconcile { version, instance } => (Request::Reconcile { version, instance }, None),
            F::Inspect {
                version,
                instance,
                incarnation,
                sequence,
            } => (
                Request::Inspect {
                    version,
                    instance,
                    incarnation,
                    sequence,
                },
                None,
            ),
            F::Cancel {
                version,
                instance,
                incarnation,
                sequence,
            } => (
                Request::Cancel {
                    version,
                    instance,
                    incarnation,
                    sequence,
                },
                None,
            ),
            F::Start {
                version,
                instance,
                incarnation,
                sequence,
                policy,
                bundle,
                catalogue,
                resource_model,
                workload,
            } => (
                Request::Start {
                    version,
                    instance,
                    incarnation,
                    sequence,
                    policy,
                    bundle,
                    workload: Workload::Film(workload),
                },
                Some((catalogue, resource_model)),
            ),
        };
        let result = self.handle_shared(request.clone(), peer_pid, ids)?;
        self.cancel_accepted(request, result)
    }

    fn handle_shared(
        self: &Arc<Self>,
        request: Request,
        peer_pid: u32,
        film_ids: Option<(String, String)>,
    ) -> Result<ResultBody, ErrorCode> {
        if request.instance() != self.config.instance {
            return Err(ErrorCode::WrongInstance);
        }
        self.backend.check_caller(peer_pid)?;
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        match request {
            Request::Reconcile { .. } => {
                if let Some((config, _)) = &self.film {
                    return Ok(ResultBody::Film(Box::new(
                        crate::film::ResultBody::Capability {
                            capability: "film-measurement-only".into(),
                            instance: self.config.instance.clone(),
                            incarnation: data.registry.incarnation.clone(),
                            next_sequence: data
                                .registry
                                .watermark
                                .checked_add(1)
                                .ok_or(ErrorCode::Capacity)?,
                            policy: self.policy.clone(),
                            bundle: self.bundle.clone(),
                            catalogue: config.catalogue_sha256.clone(),
                            resource_model: config.resource_model_sha256.clone(),
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
                            active: data
                                .registry
                                .active
                                .and_then(|s| data.registry.records.get(&s))
                                .and_then(|r| r.film.as_ref().map(|f| f.receipt(&r.receipt))),
                        },
                    )));
                }
                Ok(ResultBody::Capability {
                    capability: "qualification-only".into(),
                    instance: self.config.instance.clone(),
                    incarnation: data.registry.incarnation.clone(),
                    next_sequence: data
                        .registry
                        .watermark
                        .checked_add(1)
                        .ok_or(ErrorCode::Capacity)?,
                    policy: self.policy.clone(),
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
                })
            }
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
                        || record
                            .film
                            .as_ref()
                            .map(|f| (&f.catalogue, &f.resource_model))
                            != film_ids.as_ref().map(|(c, m)| (c, m))
                    {
                        return Err(ErrorCode::Conflict);
                    }
                    return Ok(record.result_body());
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
                if policy != self.policy {
                    return Err(ErrorCode::IncompatiblePolicy);
                }
                if bundle != self.bundle {
                    return Err(ErrorCode::IncompatibleBundle);
                }
                self.backend
                    .admission_ready(data.registry.parent_identity.as_ref())?;
                let time = now()?;
                let launch_id = random_id()?;
                let film = match (&self.film, &workload, film_ids) {
                    (None, Workload::Film(_), _) | (Some(_), _, None) => {
                        return Err(ErrorCode::InvalidRequest);
                    }
                    (
                        Some((config, documents)),
                        Workload::Film(workload),
                        Some((catalogue, resource_model)),
                    ) => {
                        if catalogue != config.catalogue_sha256 {
                            return Err(ErrorCode::IncompatibleCatalogue);
                        }
                        if resource_model != config.resource_model_sha256 {
                            return Err(ErrorCode::IncompatibleResourceModel);
                        }
                        let (fixture, plan) =
                            documents.plan(&workload.fixture_id, self.config.memory_bytes)?;
                        let manifest = crate::film::CanonicalManifest {
                            fixture: fixture.clone(),
                            recipe: documents.catalogue.recipe.clone(),
                            catalogue: catalogue.clone(),
                            resource_model: resource_model.clone(),
                            procedure: crate::film::PROCEDURE.into(),
                            bundle: bundle.clone(),
                            policy: policy.clone(),
                            plan: plan.clone(),
                        };
                        let grant = crate::film::EngineGrant {
                            version: 2,
                            kind: "film-measurement-grant".into(),
                            launch_id: launch_id.clone(),
                            manifest: crate::film::hash(&manifest)?,
                            bundle: bundle.clone(),
                            numerical_bundle: documents.catalogue.numerical_bundle.clone(),
                            recipe: documents.catalogue.recipe.clone(),
                            procedure: crate::film::PROCEDURE.into(),
                            fixture,
                            input_icc_sha256: documents.catalogue.input_icc_sha256.clone(),
                            output_icc_sha256: documents.catalogue.output_icc_sha256.clone(),
                            plan,
                        };
                        Some(crate::film::Captured {
                            catalogue,
                            resource_model,
                            grant,
                            phase: crate::film::Phase::Preparing,
                            stage_release_intent: false,
                            engine_release_intent: false,
                            grant_file: None,
                            snapshot: None,
                            result_file: None,
                            result: None,
                            detail: None,
                        })
                    }
                    (None, _, None) => None,
                    _ => return Err(ErrorCode::InvalidRequest),
                };
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
                        deadline_unix_ms: time
                            .checked_add(if film.is_some() { 900000 } else { 30000 })
                            .ok_or(ErrorCode::Capacity)?,
                        outcome: None,
                        runtime: Some(Runtime {
                            launch_id: launch_id.clone(),
                            container_id: None,
                            attempt_unit: format!(
                                "slipstreamprocessing{}-{}.slice",
                                self.config.instance, launch_id
                            ),
                        }),
                        limits: self.config.limits(),
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
                    stop_confirmed: false,
                    settled_at_unix_ms: None,
                    film,
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
                Ok(record.result_body())
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
                Ok(record.result_body())
            }
        }
    }

    pub(crate) fn cancel(
        self: &Arc<Self>,
        request: Request,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        let result = self.handle(request.clone(), peer_pid)?;
        self.cancel_accepted(request, result)
    }

    fn cancel_accepted(
        &self,
        request: Request,
        result: ResultBody,
    ) -> Result<ResultBody, ErrorCode> {
        let Request::Cancel { sequence, .. } = request else {
            return Ok(result);
        };
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        let mut next = data.registry.clone();
        let record = next.records.get_mut(&sequence).ok_or(ErrorCode::Expired)?;
        if record.receipt.state == State::Settled || record.receipt.outcome.is_some() {
            return Ok(record.result_body());
        }
        record.receipt.cancellation_requested = true;
        let result = record.result_body();
        persist(Path::new(&self.config.root), &next)?;
        data.registry = next;
        Ok(result)
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
        record.stop_confirmed |= previous.stop_confirmed;
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
        let gate = match gate {
            Ok(gate) => gate,
            Err(_) => {
                self.backend.discover(&mut record)?;
                self.update(&record)?;
                return self.settle(record, Some(Outcome::Interrupted));
            }
        };
        let mut gate = match gate {
            crate::backend::Gate::Native(file) => file,
            crate::backend::Gate::Film(session) => return self.execute_film(record, session),
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

    fn interrupted(&self, record: &Record) -> Result<Option<Outcome>, ErrorCode> {
        if self
            .record(record.receipt.sequence)?
            .receipt
            .cancellation_requested
        {
            Ok(Some(Outcome::Cancelled))
        } else if now()? >= record.receipt.deadline_unix_ms {
            Ok(Some(Outcome::Deadline))
        } else {
            Ok(None)
        }
    }
    fn film_wait<T>(
        &self,
        record: &Record,
        mut poll: impl FnMut() -> Result<Option<T>, ErrorCode>,
    ) -> Result<(Option<T>, Option<Outcome>), ErrorCode> {
        loop {
            if let Some(reason) = self.interrupted(record)? {
                return Ok((None, Some(reason)));
            }
            if !self.backend.live(record)?.running {
                return Ok((None, None));
            }
            if let Some(value) = poll()? {
                return Ok((Some(value), None));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn execute_film(
        &self,
        mut record: Record,
        mut session: Box<crate::staging::Session>,
    ) -> Result<(), ErrorCode> {
        use crate::{film, staging};
        use std::os::fd::AsRawFd;
        let execution = (|| -> Result<Option<Outcome>, ErrorCode> {
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .stage_release_intent = true;
            record.released = true;
            self.update(&record)?;
            crate::faults::at(
                &self.config,
                &record,
                crate::faults::Phase::StageReleaseIntent,
            )?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            self.backend.release(&mut record, |r| self.update(r))?;
            let (connected, reason) = self.film_wait(&record, || {
                if session.started.elapsed() > Duration::from_secs(10) {
                    return Err(ErrorCode::Uncertain);
                }
                let live = self.backend.live(&record)?;
                Ok(session.accept(live.pid)?.then_some(()))
            })?;
            if connected.is_none() {
                return Ok(reason);
            }
            {
                let data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
                if data.registry.records[&record.receipt.sequence]
                    .receipt
                    .cancellation_requested
                {
                    return Ok(Some(Outcome::Cancelled));
                }
                if now()? >= record.receipt.deadline_unix_ms {
                    return Ok(Some(Outcome::Deadline));
                }
                self.backend.verify_film_bootstrap(&record, session.pid)?;
                if crate::faults::retain_snapshot_writer(&self.config, &record)? {
                    session.retain_snapshot_writer()?;
                }
                session.offer()?;
            }
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Staging;
            record.receipt.state = State::Running;
            self.update(&record)?;
            let fd = session
                .connection
                .as_ref()
                .ok_or(ErrorCode::Uncertain)?
                .as_raw_fd();
            let (ack, reason) = self.film_wait(&record, || {
                staging::receive::<film::StageAck>(fd).map_err(|_| ErrorCode::Uncertain)
            })?;
            let Some((ack, rights)) = ack else {
                return Ok(reason);
            };
            if !rights.is_empty() || ack != session.offer.ack() {
                return Err(ErrorCode::Uncertain);
            }
            crate::faults::at(&self.config, &record, crate::faults::Phase::StageAck)?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            let leaf = self.backend.pause_film(&mut record, |r| self.update(r))?;
            if self.backend.live(&record)?.pid != session.pid {
                return Err(ErrorCode::Uncertain);
            }
            match session.audit(&record, &leaf) {
                Ok(()) => {}
                Err(ErrorCode::InvalidRequest) => {
                    record.film.as_mut().ok_or(ErrorCode::Uncertain)?.detail =
                        Some(film::Detail::SourceMismatch);
                    self.update(&record)?;
                    return Ok(Some(Outcome::Interrupted));
                }
                Err(error) => return Err(error),
            }
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Sealed;
            self.update(&record)?;
            crate::faults::at(&self.config, &record, crate::faults::Phase::SnapshotSealed)?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .engine_release_intent = true;
            self.update(&record)?;
            crate::faults::at(
                &self.config,
                &record,
                crate::faults::Phase::EngineReleaseIntent,
            )?;
            if let Some(reason) = self.interrupted(&record)? {
                return Ok(Some(reason));
            }
            session.unlink_endpoint()?;
            self.backend.release(&mut record, |r| self.update(r))?;
            let permit = film::Permit {
                version: 2,
                kind: "engine-permit".into(),
                launch_id: record.launch_id.clone(),
                grant_sha256: session.offer.grant_sha256.clone(),
            };
            {
                let data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
                if data.registry.records[&record.receipt.sequence]
                    .receipt
                    .cancellation_requested
                {
                    return Ok(Some(Outcome::Cancelled));
                }
                if now()? >= record.receipt.deadline_unix_ms {
                    return Ok(Some(Outcome::Deadline));
                }
                staging::send(fd, &permit, &[]).map_err(|_| ErrorCode::Uncertain)?;
            }
            let (started, reason) = self.film_wait(&record, || {
                staging::receive::<film::Permit>(fd).map_err(|_| ErrorCode::Uncertain)
            })?;
            let Some((started, rights)) = started else {
                return Ok(reason);
            };
            if !rights.is_empty()
                || started
                    != (film::Permit {
                        kind: "engine-started".into(),
                        ..permit
                    })
            {
                return Err(ErrorCode::Uncertain);
            }
            record.film.as_mut().ok_or(ErrorCode::Uncertain)?.phase = film::Phase::Engine;
            self.update(&record)?;
            drop(session.connection.take());
            loop {
                if !self.backend.live(&record)?.running {
                    return Ok(None);
                }
                if let Some(reason) = self.interrupted(&record)? {
                    return Ok(Some(reason));
                }
                thread::sleep(Duration::from_millis(50));
            }
        })();
        match execution {
            Ok(reason) => self.settle_inner(record, reason, Some(session)),
            Err(error) if record.manager_pending.is_some() => Err(error),
            Err(_) => self.settle_inner(record, Some(Outcome::Interrupted), Some(session)),
        }
    }
    fn settle(&self, record: Record, requested: Option<Outcome>) -> Result<(), ErrorCode> {
        self.settle_inner(record, requested, None)
    }
    fn settle_inner(
        &self,
        mut record: Record,
        requested: Option<Outcome>,
        session: Option<Box<crate::staging::Session>>,
    ) -> Result<(), ErrorCode> {
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
            drop(session);
            self.backend
                .cleanup(&mut record, |record| self.update(record))?;
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
            let worker = if record.film.is_some() {
                let result = self
                    .backend
                    .film_result(&record, session.as_ref().map(|s| &s.result));
                let captured = record.film.as_mut().ok_or(ErrorCode::Uncertain)?;
                match result {
                    Ok(result) => {
                        captured.result = result;
                        captured.result.as_ref().map(|r| r.outcome())
                    }
                    Err(ErrorCode::InvalidRequest) => {
                        captured.detail = Some(crate::film::Detail::ArtifactInvalid);
                        None
                    }
                    Err(error) => return Err(error),
                }
            } else {
                self.backend.worker_outcome(&record)?
            };
            let reason =
                record
                    .termination_reason
                    .or(if !record.released { requested } else { None });
            let mut outcome = classify(&evidence, worker, reason);
            if let Some(film) = record.film.as_mut() {
                if outcome != Outcome::Oom {
                    if let Some(result) = &film.result {
                        let expected = match result.outcome() {
                            Outcome::Completed => 0,
                            Outcome::AllocationFailed => 20,
                            Outcome::StorageFull => 21,
                            Outcome::Deadline => 76,
                            _ => 75,
                        };
                        if evidence.exit_code == Some(expected) {
                            outcome = result.outcome();
                            film.detail = result.detail();
                        }
                    } else if film.detail.is_some() {
                        outcome = Outcome::EngineFailed;
                    }
                }
                if outcome == Outcome::Oom {
                    film.detail = None;
                }
                if evidence.exit_code.is_some() {
                    film.phase = crate::film::Phase::ExecutionFinished;
                }
            }
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
        drop(session);
        if record.film.as_ref().is_some_and(|f| f.result.is_some()) {
            crate::faults::at(&self.config, &record, crate::faults::Phase::ValidatedResult)?;
        }
        crate::faults::at(&self.config, &record, crate::faults::Phase::Evidence)?;
        self.backend
            .cleanup(&mut record, |record| self.update(record))?;
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
    if registry.version != config.version
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
            || record.film.is_some() != (config.version == 2)
            || (record.stop_confirmed
                && (record.unit_invocation.is_none()
                    || record.cgroup_inode.is_none()
                    || record.receipt.outcome.is_none()
                    || record
                        .receipt
                        .evidence
                        .as_ref()
                        .is_none_or(|e| e.populated != Some(false))))
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
        if let Some(film) = &record.film
            && (film.grant.validate().is_err()
                || film.grant.launch_id != record.launch_id
                || !hex(&film.catalogue, 64)
                || !hex(&film.resource_model, 64)
                || record.receipt.workload
                    != Workload::Film(crate::film::Workload {
                        kind: "film-fixture".into(),
                        fixture_id: film.grant.fixture.id.clone(),
                    }))
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
            stop_confirmed: false,
            settled_at_unix_ms: Some(1000),
            film: None,
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
    fn legacy_records_do_not_invent_confirmed_stops() {
        let mut value = serde_json::to_value(record(1, State::Settled)).unwrap();
        value.as_object_mut().unwrap().remove("stop_confirmed");
        let restored: Record = serde_json::from_value(value).unwrap();
        assert!(!restored.stop_confirmed);
        let mut confirmed = serde_json::to_value(restored).unwrap();
        confirmed["stop_confirmed"] = serde_json::json!(true);
        let restored: Record = serde_json::from_value(confirmed).unwrap();
        assert!(restored.stop_confirmed);
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
    #[test]
    fn maximal_film_receipts_fit_all_fixed_protocol_and_journal_bounds() {
        use crate::film;
        let mut grant = film::test_grant();
        grant.fixture.width = 9568;
        grant.fixture.height = 9568;
        grant.fixture.source = film::Source::DevelopmentTiff {
            bytes: 2 * 1024 * 1024 * 1024,
            sha256: "e".repeat(64),
        };
        let case = film::ModelCase {
            fixture_id: grant.fixture.id.clone(),
            stages: film::STAGES
                .iter()
                .map(|stage| film::StageBounds {
                    stage: *stage,
                    runtime: film::Bound::Unknown,
                    native: film::Bound::Unknown,
                    allocator_retention: film::Bound::Unknown,
                    kernel: film::Bound::Unknown,
                })
                .collect(),
        };
        grant.plan = film::plan(&grant.fixture, &case, 32 * 1024 * 1024 * 1024).unwrap();
        // Schema maxima intentionally over-approximate the compiled planner's current known subtotal.
        grant.plan.prediction = film::Prediction::Unqualified {
            known_required_bytes: u64::MAX,
            known_terms_exceed_limit: true,
            missing: film::STAGES
                .iter()
                .flat_map(|stage| {
                    [
                        film::Term::OwnedArrays,
                        film::Term::Runtime,
                        film::Term::Native,
                        film::Term::AllocatorRetention,
                        film::Term::Kernel,
                    ]
                    .map(|term| film::MissingTerm {
                        stage: *stage,
                        term,
                    })
                })
                .collect(),
        };
        let mut registry = registry();
        registry.version = 2;
        registry.watermark = u64::MAX;
        registry.records = registry
            .records
            .into_values()
            .enumerate()
            .map(|(index, mut r)| {
                let seq = u64::MAX - index as u64;
                r.receipt.sequence = seq;
                (seq, r)
            })
            .collect();
        for record in registry.records.values_mut() {
            grant.launch_id = record.launch_id.clone();
            record.receipt.workload = Workload::Film(film::Workload {
                kind: "film-fixture".into(),
                fixture_id: grant.fixture.id.clone(),
            });
            record.receipt.limits = film::limits(32 * 1024 * 1024 * 1024);
            record.receipt.accepted_at_unix_ms = u64::MAX - 900000;
            record.receipt.deadline_unix_ms = u64::MAX;
            record.receipt.runtime.as_mut().unwrap().container_id =
                Some(format!("{:064x}", record.receipt.sequence));
            let r = &grant.fixture.reference;
            record.film = Some(film::Captured {
                catalogue: "b".repeat(64),
                resource_model: "c".repeat(64),
                grant: grant.clone(),
                phase: film::Phase::ExecutionFinished,
                stage_release_intent: true,
                engine_release_intent: true,
                grant_file: Some(film::FileIdentity {
                    device: u64::MAX,
                    inode: u64::MAX,
                }),
                snapshot: Some(film::FileIdentity {
                    device: u64::MAX,
                    inode: u64::MAX,
                }),
                result_file: Some(film::FileIdentity {
                    device: u64::MAX,
                    inode: u64::MAX,
                }),
                detail: Some(film::Detail::UnsupportedInput),
                result: Some(film::WorkerResult::Success(film::WorkerSuccess {
                    version: 2,
                    kind: "film-measurement-result".into(),
                    outcome: Outcome::Completed,
                    launch_id: record.launch_id.clone(),
                    manifest: grant.manifest.clone(),
                    plan_sha256: film::hash(&grant.plan).unwrap(),
                    artifact: film::Artifact {
                        input_pixels_sha256: r.input_pixels_sha256.clone(),
                        film_pixels_sha256: r.film_pixels_sha256.clone(),
                        jpeg_sha256: r.jpeg_sha256.clone(),
                        jpeg_bytes: r.jpeg_bytes,
                        width: grant.fixture.width,
                        height: grant.fixture.height,
                        icc_sha256: grant.output_icc_sha256.clone(),
                        reference_evidence_sha256: r.evidence_sha256.clone(),
                    },
                    execution_us: u64::MAX,
                    stages: film::STAGES
                        .iter()
                        .map(|stage| film::Timing {
                            stage: *stage,
                            elapsed_us: u64::MAX,
                            reclaim_us: u64::MAX,
                        })
                        .collect(),
                })),
            });
            record.receipt.outcome = Some(Outcome::Completed);
            let events = Events {
                oom: u64::MAX,
                oom_kill: u64::MAX,
                oom_group_kill: u64::MAX,
                local_oom: u64::MAX,
                local_oom_kill: u64::MAX,
                local_oom_group_kill: u64::MAX,
            };
            record.receipt.evidence = Some(Evidence {
                peak_bytes: u64::MAX,
                exit_code: Some(0),
                docker_oom_killed: Some(false),
                attempt_before: Some(events.clone()),
                attempt_after: Some(events.clone()),
                parent_before: Some(events.clone()),
                parent_after: Some(events),
                populated: Some(false),
            });
        }
        let bytes = serde_json::to_vec(&registry).unwrap();
        let record = registry.records.get(&u64::MAX).unwrap();
        let captured = record.film.as_ref().unwrap();
        let grant_bytes = film::canonical(&captured.grant).unwrap();
        let response = serde_json::to_vec(&Response::Result {
            version: 2,
            result: Box::new(record.result_body()),
        })
        .unwrap();
        let worker = serde_json::to_vec(captured.result.as_ref().unwrap()).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(wire["result"]["kind"], "receipt");
        assert_eq!(
            wire["result"]["receipt"]["workload"]["kind"],
            "film-fixture"
        );
        assert!(bytes.len() < 4 * 1024 * 1024, "journal {}", bytes.len());
        assert!(grant_bytes.len() <= film::FRAME);
        assert!(response.len() <= RESPONSE_BYTES);
        assert!(worker.len() <= film::FRAME - 4);
        println!(
            "maximal Film bounds: journal256={} grant={} response={} worker={}",
            bytes.len(),
            grant_bytes.len(),
            response.len(),
            worker.len()
        );
    }
}
