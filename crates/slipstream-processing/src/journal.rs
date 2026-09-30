use crate::{
    backend::{Backend, secure_directory},
    instance_claim::claim_instance,
    protocol::*,
};
use serde::{Deserialize, Serialize};
#[path = "journal_film.rs"]
mod journal_film;
#[path = "qualification.rs"]
mod qualification;
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
            if film.grant.plan.qualified().is_some() {
                return ResultBody::Qualified(Box::new(crate::qualified::ResultBody::Receipt {
                    receipt: film.qualified_receipt(&self.receipt),
                }));
            }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invalidations: Option<Vec<crate::qualified::Invalidation>>,
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
    qualified: Option<(
        crate::qualified::Config,
        crate::qualified::Documents,
        String,
    )>,
    policy: String,
    _lock: File,
    _instance_claim: File,
}

impl Executor {
    pub fn open(config: Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        Self::open_common(config, None, None)
    }

    pub fn open_film(config: crate::film::Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        Self::open_common(config.authority(), Some(config), None)
    }

    pub fn open_qualified(config: crate::qualified::Config) -> Result<Arc<Self>, ErrorCode> {
        config.validate()?;
        Self::open_common(config.authority(), None, Some(config))
    }

    fn open_common(
        config: Config,
        film_config: Option<crate::film::Config>,
        qualified_config: Option<crate::qualified::Config>,
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
        let qualified = qualified_config
            .map(|c| {
                let documents = crate::qualified::Documents::load(&c)?;
                let launcher = crate::environment::launcher_identity()?;
                Ok::<_, ErrorCode>((c, documents, launcher))
            })
            .transpose()?;
        // The qualification, film, and qualified executors keep the claim for
        // their lifetime and ignore the lease: removing a refused start's
        // claim is the photo executor's policy. Taking the guard here is
        // infallible, so no fallible step can run while the lease is armed.
        let _instance_claim = claim_instance(&config)?.take();
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
        // Expected v3 identity is not readiness. Actual image/environment
        // readback gates new work, while old exact records can still recover.
        let image_id = if let Some((_, documents, _)) = &qualified {
            documents.envelope.image.clone()
        } else {
            backend.image()?
        };
        let (bundle, policy) = if let Some((qualified_config, documents, _)) = &qualified {
            (
                crate::film::hash(
                    &serde_json::json!({"profile":crate::qualified::PROFILE,"image":image_id,"numerical_bundle":documents.catalogue.numerical_bundle}),
                )?,
                qualified_config.policy(&image_id)?,
            )
        } else if let Some((film_config, documents)) = &film {
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
        let mut registry = load(root)?.unwrap_or(Registry {
            version: config.version,
            instance: config.instance.clone(),
            incarnation: random_id()?,
            watermark: 0,
            parent_pending: false,
            parent_identity: None,
            active: None,
            records: BTreeMap::new(),
            invalidations: qualified.as_ref().map(|_| Vec::new()),
        });
        validate_registry(&registry, &config)?;
        qualification::restore(&mut registry)?;
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
            qualified,
            policy,
            _lock: lock,
            _instance_claim,
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
        self.film.is_some() || self.qualified.is_some()
    }

    pub(crate) fn handle_qualified(
        self: &Arc<Self>,
        request: crate::qualified::Request,
        peer_pid: u32,
    ) -> Result<ResultBody, ErrorCode> {
        let (request, ids) = request.into_shared();
        let result = self.handle_shared(request.clone(), peer_pid, ids)?;
        self.cancel_accepted(request, result)
    }

    fn qualified_ready(&self) -> Result<(), ErrorCode> {
        if let Some((_, documents, launcher)) = &self.qualified {
            (|| {
                let image = self.backend.image()?;
                self.backend
                    .verify_film_image(&image, &documents.catalogue)?;
                documents
                    .envelope
                    .matches(&image, launcher, &self.backend.environment()?)
            })()
            .map_err(|_| ErrorCode::Unavailable)?;
        }
        Ok(())
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
        let mut data = self.lock()?;
        match request {
            Request::Reconcile { .. } => {
                let incarnation = data.registry.incarnation.clone();
                let next_sequence = data
                    .registry
                    .watermark
                    .checked_add(1)
                    .ok_or(ErrorCode::Capacity)?;
                if let Some((config, documents, _)) = &self.qualified {
                    let invalidations = data
                        .registry
                        .invalidations
                        .as_ref()
                        .ok_or(ErrorCode::Uncertain)?;
                    let ready = matches!(self.availability(&data), Availability::Available)
                        && self.qualified_ready().is_ok();
                    let availability = qualification::availability(
                        ready,
                        &documents.envelope.cases,
                        &config.envelope_sha256,
                        invalidations,
                    );
                    return Ok(ResultBody::Qualified(Box::new(
                        crate::qualified::ResultBody::Capability {
                            capability: "film-qualified-fixtures-only".into(),
                            instance: self.config.instance.clone(),
                            incarnation,
                            next_sequence,
                            policy: self.policy.clone(),
                            bundle: self.bundle.clone(),
                            catalogue: config.catalogue_sha256.clone(),
                            envelope: config.envelope_sha256.clone(),
                            availability,
                            active: data
                                .registry
                                .active
                                .and_then(|s| data.registry.records.get(&s))
                                .and_then(|r| {
                                    r.film.as_ref().map(|f| f.qualified_receipt(&r.receipt))
                                }),
                        },
                    )));
                }
                if let Some((config, _)) = &self.film {
                    return Ok(ResultBody::Film(Box::new(
                        crate::film::ResultBody::Capability {
                            capability: "film-measurement-only".into(),
                            instance: self.config.instance.clone(),
                            incarnation,
                            next_sequence,
                            policy: self.policy.clone(),
                            bundle: self.bundle.clone(),
                            catalogue: config.catalogue_sha256.clone(),
                            resource_model: config.resource_model_sha256.clone(),
                            availability: self.availability(&data),
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
                    incarnation,
                    next_sequence,
                    policy: self.policy.clone(),
                    bundle: self.bundle.clone(),
                    availability: self.availability(&data),
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
                if self.qualified.is_some() {
                    self.backend
                        .verify_parent(data.registry.parent_identity.as_ref())
                        .map_err(|_| ErrorCode::Unavailable)?;
                }
                self.qualified_ready()?;
                let time = now()?;
                let launch_id = random_id()?;
                let film = if let Some((config, documents, _)) = &self.qualified {
                    let Workload::Film(workload) = &workload else {
                        return Err(ErrorCode::InvalidRequest);
                    };
                    let (catalogue, envelope) = film_ids.ok_or(ErrorCode::InvalidRequest)?;
                    if catalogue != config.catalogue_sha256 {
                        return Err(ErrorCode::IncompatibleCatalogue);
                    }
                    if envelope != config.envelope_sha256 {
                        return Err(ErrorCode::IncompatibleEnvelope);
                    }
                    let invalidations = data
                        .registry
                        .invalidations
                        .as_ref()
                        .ok_or(ErrorCode::Uncertain)?;
                    let (fixture, plan) = qualification::plan(
                        documents,
                        invalidations,
                        &workload.fixture_id,
                        &envelope,
                        self.config.memory_bytes,
                    )?;
                    let manifest = crate::qualified::CanonicalManifest {
                        fixture: fixture.clone(),
                        recipe: documents.catalogue.recipe.clone(),
                        catalogue: catalogue.clone(),
                        envelope: envelope.clone(),
                        procedure: crate::film::PROCEDURE.into(),
                        bundle: bundle.clone(),
                        policy: policy.clone(),
                        plan: plan.clone(),
                    };
                    let grant = crate::film::EngineGrant {
                        version: 3,
                        kind: "film-qualified-grant".into(),
                        launch_id: launch_id.clone(),
                        manifest: crate::film::hash(&manifest)?,
                        bundle: bundle.clone(),
                        numerical_bundle: documents.catalogue.numerical_bundle.clone(),
                        recipe: documents.catalogue.recipe.clone(),
                        procedure: crate::film::PROCEDURE.into(),
                        fixture,
                        input_icc_sha256: documents.catalogue.input_icc_sha256.clone(),
                        output_icc_sha256: documents.catalogue.output_icc_sha256.clone(),
                        plan: crate::film::ExecutionPlan::Qualified(plan),
                    };
                    grant.validate()?;
                    Some(crate::film::Captured {
                        catalogue,
                        resource_model: envelope,
                        grant,
                        phase: crate::film::Phase::Preparing,
                        stage_release_intent: false,
                        engine_release_intent: false,
                        grant_file: None,
                        snapshot: None,
                        result_file: None,
                        result: None,
                        detail: None,
                        qualification_failure: None,
                        qualification_observation_valid: Some(true),
                    })
                } else {
                    match (&self.film, &workload, film_ids) {
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
                                grant: grant.map_plan(crate::film::ExecutionPlan::Measurement),
                                phase: crate::film::Phase::Preparing,
                                stage_release_intent: false,
                                engine_release_intent: false,
                                grant_file: None,
                                snapshot: None,
                                result_file: None,
                                result: None,
                                detail: None,
                                qualification_failure: None,
                                qualification_observation_valid: None,
                            })
                        }
                        (None, _, None) => None,
                        _ => return Err(ErrorCode::InvalidRequest),
                    }
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
        let mut data = self.lock()?;
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

    fn availability(&self, data: &Data) -> Availability {
        if data.available
            && self
                .backend
                .admission_ready(data.registry.parent_identity.as_ref())
                .is_ok()
        {
            Availability::Available
        } else {
            Availability::Blocked
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Data>, ErrorCode> {
        self.data.lock().map_err(|_| ErrorCode::Uncertain)
    }

    fn record(&self, sequence: u64) -> Result<Record, ErrorCode> {
        self.lock()?
            .registry
            .records
            .get(&sequence)
            .cloned()
            .ok_or(ErrorCode::Uncertain)
    }

    fn update(&self, record: &Record) -> Result<(), ErrorCode> {
        let mut data = self.lock()?;
        let mut next = data.registry.clone();
        let previous = next
            .records
            .get(&record.receipt.sequence)
            .ok_or(ErrorCode::Uncertain)?;
        let mut record = record.clone();
        record.receipt.cancellation_requested |= previous.receipt.cancellation_requested;
        record.termination_reason = previous.termination_reason.or(record.termination_reason);
        record.stop_confirmed |= previous.stop_confirmed;
        if previous
            .film
            .as_ref()
            .is_some_and(|film| film.qualification_observation_valid == Some(false))
        {
            record
                .film
                .as_mut()
                .ok_or(ErrorCode::Uncertain)?
                .qualification_observation_valid = Some(false);
        }
        if let Some(outcome) = previous.receipt.outcome
            && outcome != Outcome::Unknown
        {
            record.receipt.outcome = Some(outcome);
        }
        qualification::assess(&mut next, &mut record)?;
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
        if let Ok(mut data) = self.lock() {
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
        self.backend.verify_parent(parent.as_ref())?;
        self.backend.scan_unowned(&records)?;
        for mut record in records {
            if record.receipt.state == State::Settled {
                continue;
            }
            self.backend.discover(&mut record)?;
            self.update(&record)?;
            let requested = self.interrupted(&record)?.unwrap_or(Outcome::Interrupted);
            self.settle(record, Some(requested))?;
        }
        {
            let mut data = self.lock()?;
            data.registry.parent_pending = true;
            persist(Path::new(&self.config.root), &data.registry)?;
        }
        let identity = self.backend.prepare_parent(parent.as_ref())?;
        {
            let mut data = self.lock()?;
            data.registry.parent_identity = Some(identity);
            data.registry.parent_pending = false;
            persist(Path::new(&self.config.root), &data.registry)?;
            data.available = true;
        }
        Ok(())
    }

    fn verify_parent(&self) -> Result<(), ErrorCode> {
        let identity = self
            .lock()?
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
        if let Some(reason) = self.interrupted(&record)? {
            return self.settle(record, Some(reason));
        }
        record.released = true;
        self.update(&record)?;
        crate::faults::at(&self.config, &record, crate::faults::Phase::ReleaseIntent)?;
        // Holding the instance journal mutex closes the cancel/release race.
        {
            let data = self.lock()?;
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
            if !self.backend.live(&record)?.running {
                return self.settle(record, None);
            }
            if let Some(reason) = self.interrupted(&record)? {
                return self.settle(record, Some(reason));
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
        let has_container = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_ref())
            .is_some();
        if record.receipt.outcome.is_some()
            && record
                .receipt
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.populated == Some(false))
        {
            self.backend.validate_terminal_evidence(&record)?;
            drop(session);
            return self.finish_settlement(&mut record);
        }
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
                terminal_snapshot: None,
            });
        }
        let response = Response::Result {
            version: self.config.version,
            result: Box::new(record.result_body()),
        };
        if serde_json::to_vec(&response)
            .map_err(|_| ErrorCode::Uncertain)?
            .len()
            > RESPONSE_BYTES
        {
            return Err(ErrorCode::Uncertain);
        }
        self.update(&record)?;
        drop(session);
        if record.film.as_ref().is_some_and(|f| f.result.is_some()) {
            crate::faults::at(&self.config, &record, crate::faults::Phase::ValidatedResult)?;
        }
        crate::faults::at(&self.config, &record, crate::faults::Phase::Evidence)?;
        self.finish_settlement(&mut record)
    }

    /// Remove the attempt boundary and settle the receipt durably.
    fn finish_settlement(&self, record: &mut Record) -> Result<(), ErrorCode> {
        self.backend.cleanup(record, |record| self.update(record))?;
        record.receipt.cleanup = Cleanup::Complete;
        record.receipt.state = State::Settled;
        record.settled_at_unix_ms = Some(now()?);
        self.update(record)
    }
}

pub(crate) fn classify(
    evidence: &Evidence,
    worker: Option<Outcome>,
    requested: Option<Outcome>,
) -> Outcome {
    if qualification::attempt_oom_killed(evidence)
        && (evidence.docker_oom_killed == Some(true)
            || (evidence.exit_code == Some(137) && qualification::owned_limit_pressure(evidence)))
    {
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
        || registry.records.len() > if config.version == 3 { 256 } else { 257 }
        || (!registry.records.is_empty() && registry.parent_identity.is_none())
        || registry
            .parent_identity
            .as_ref()
            .is_some_and(|identity| !hex(&identity.invocation, 32) || identity.inode == 0)
    {
        return Err(ErrorCode::Uncertain);
    }
    match (&registry.invalidations, config.version) {
        (Some(entries), 3) if entries.len() <= crate::qualified::INVALIDATIONS => {
            let mut pairs = std::collections::BTreeSet::new();
            for entry in entries {
                if !hex(&entry.envelope, 64)
                    || !hex(&entry.fixture_id, 32)
                    || entry.incarnation != registry.incarnation
                    || entry.sequence == 0
                    || entry.sequence > registry.watermark
                    || !pairs.insert((&entry.envelope, &entry.fixture_id))
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        (None, 1 | 2) => {}
        _ => return Err(ErrorCode::Uncertain),
    }
    let mut active = None;
    for (sequence, record) in &registry.records {
        if *sequence == 0
            || *sequence > registry.watermark
            || *sequence != record.receipt.sequence
            || record.receipt.incarnation != registry.incarnation
            || !hex(&record.receipt.policy, 64)
            || !hex(&record.receipt.bundle, 64)
            || !record
                .image_id
                .strip_prefix("sha256:")
                .is_some_and(|id| hex(id, 64))
            || record
                .unit_invocation
                .as_ref()
                .is_some_and(|id| !hex(id, 32))
            || record.termination_reason.is_some_and(|reason| {
                !matches!(
                    reason,
                    Outcome::Cancelled | Outcome::Deadline | Outcome::Interrupted
                )
            })
            || record.film.is_some() != matches!(config.version, 2 | 3)
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
                || film.grant.version != config.version
                || film.grant.launch_id != record.launch_id
                || film.grant.bundle != record.receipt.bundle
                || !hex(&film.catalogue, 64)
                || !hex(&film.resource_model, 64)
                || (config.version != 3 && film.qualification_failure.is_some())
                || film.qualification_observation_valid.is_some() != (config.version == 3)
                || film.grant.plan.qualified().is_some_and(|plan| {
                    plan.envelope_sha256 != film.resource_model
                        || record.receipt.limits != crate::film::limits(plan.attempt_limit_bytes)
                })
                || film
                    .result
                    .as_ref()
                    .is_some_and(|result| result.validate(&film.grant).is_err())
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
#[path = "journal_tests.rs"]
pub(crate) mod tests;