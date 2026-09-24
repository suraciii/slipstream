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
        let (instance_claim, _fresh) = claim_instance(&config)?;
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
        restore_qualifications(&mut registry)?;
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
        let mut data = self.data.lock().map_err(|_| ErrorCode::Uncertain)?;
        match request {
            Request::Reconcile { .. } => {
                if let Some((config, documents, _)) = &self.qualified {
                    let invalidations = data
                        .registry
                        .invalidations
                        .as_ref()
                        .ok_or(ErrorCode::Uncertain)?;
                    let ready = data.available
                        && self
                            .backend
                            .admission_ready(data.registry.parent_identity.as_ref())
                            .is_ok()
                        && self.qualified_ready().is_ok();
                    let availability = qualified_availability(
                        ready,
                        &documents.envelope.cases,
                        &config.envelope_sha256,
                        invalidations,
                    );
                    return Ok(ResultBody::Qualified(Box::new(
                        crate::qualified::ResultBody::Capability {
                            capability: "film-qualified-fixtures-only".into(),
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
                    let (fixture, plan) = plan_qualified_start(
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
        assess_qualification(&mut next, &mut record)?;
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

    fn film_permit(
        &self,
        record: &Record,
        parent: Option<&ParentIdentity>,
        pid: u32,
    ) -> Result<(), ErrorCode> {
        self.backend.verify_film_bootstrap(record, pid)?;
        let Some((config, documents, _)) = &self.qualified else {
            return Ok(());
        };
        self.backend.verify_parent(parent)?;
        self.backend.admission_ready(parent)?;
        self.qualified_ready()?;
        let captured = record.film.as_ref().ok_or(ErrorCode::Uncertain)?;
        captured.grant.validate()?;
        let (_, expected) = documents.plan(
            &captured.grant.fixture.id,
            &config.envelope_sha256,
            record.receipt.limits.memory_bytes,
        )?;
        if captured.catalogue != config.catalogue_sha256
            || captured.resource_model != config.envelope_sha256
            || captured.grant.plan.qualified() != Some(&expected)
            || record.image_id != self.image_id
            || record.receipt.policy != self.policy
            || record.receipt.bundle != self.bundle
            || record.receipt.limits != self.config.limits()
        {
            return Err(ErrorCode::Unavailable);
        }
        self.backend.verify_film_limits(record)
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
                if let Err(error) =
                    self.film_permit(&record, data.registry.parent_identity.as_ref(), session.pid)
                {
                    drop(data);
                    if self.qualified.is_some() {
                        record
                            .film
                            .as_mut()
                            .ok_or(ErrorCode::Uncertain)?
                            .qualification_observation_valid = Some(false);
                        self.update(&record)?;
                    }
                    return Err(error);
                }
                if now()? >= record.receipt.deadline_unix_ms {
                    return Ok(Some(Outcome::Deadline));
                }
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
                if let Err(error) =
                    self.film_permit(&record, data.registry.parent_identity.as_ref(), session.pid)
                {
                    drop(data);
                    if self.qualified.is_some() {
                        record
                            .film
                            .as_mut()
                            .ok_or(ErrorCode::Uncertain)?
                            .qualification_observation_valid = Some(false);
                        self.update(&record)?;
                    }
                    return Err(error);
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
            self.backend
                .cleanup(&mut record, |record| self.update(record))?;
            record.receipt.cleanup = Cleanup::Complete;
            record.receipt.state = State::Settled;
            record.settled_at_unix_ms = Some(now()?);
            return self.update(&record);
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
    if attempt_oom_killed(evidence)
        && (evidence.docker_oom_killed == Some(true)
            || (evidence.exit_code == Some(137) && owned_limit_pressure(evidence)))
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

fn attempt_oom_killed(evidence: &Evidence) -> bool {
    evidence
        .attempt_before
        .as_ref()
        .zip(evidence.attempt_after.as_ref())
        .is_some_and(|(before, after)| {
            after
                .oom_kill
                .checked_sub(before.oom_kill)
                .is_some_and(|delta| delta > 0)
        })
}

fn owned_limit_pressure(evidence: &Evidence) -> bool {
    // Hierarchical events retain pressure at a vanished workload leaf. Only
    // local parent events exclude pressure from another subtree or ancestor.
    evidence
        .attempt_before
        .as_ref()
        .zip(evidence.attempt_after.as_ref())
        .is_some_and(|(before, after)| after.oom.checked_sub(before.oom).is_some_and(|n| n > 0))
        || evidence
            .parent_before
            .as_ref()
            .zip(evidence.parent_after.as_ref())
            .is_some_and(|(before, after)| {
                after
                    .local_oom
                    .checked_sub(before.local_oom)
                    .is_some_and(|n| n > 0)
            })
}

fn qualified_availability(
    boundary_ready: bool,
    cases: &[crate::qualified::Case],
    envelope: &str,
    invalidations: &[crate::qualified::Invalidation],
) -> crate::qualified::Availability {
    use crate::qualified::{Availability, Case};
    if !boundary_ready {
        Availability::Blocked
    } else if cases.iter().any(|case| {
        matches!(case, Case::Qualified { .. })
            && !invalidations
                .iter()
                .any(|entry| entry.envelope == envelope && entry.fixture_id == case.fixture_id())
    }) {
        Availability::Available
    } else {
        Availability::Unqualified
    }
}

fn plan_qualified_start(
    documents: &crate::qualified::Documents,
    invalidations: &[crate::qualified::Invalidation],
    fixture: &str,
    envelope: &str,
    memory: u64,
) -> Result<(crate::film::Fixture, crate::qualified::Plan), ErrorCode> {
    if invalidations.len() >= crate::qualified::INVALIDATIONS {
        return Err(ErrorCode::Capacity);
    }
    if invalidations
        .iter()
        .any(|entry| entry.envelope == envelope && entry.fixture_id == fixture)
    {
        return Err(ErrorCode::UnqualifiedEnvelope);
    }
    documents.plan(fixture, envelope, memory)
}

fn qualification_failure(record: &Record) -> Option<crate::qualified::QualificationFailure> {
    use crate::qualified::QualificationFailure as Failure;
    let captured = record.film.as_ref()?;
    let plan = captured.grant.plan.qualified()?;
    if captured.qualification_observation_valid != Some(true) {
        return None;
    }
    let outcome = record.receipt.outcome?;
    let evidence = record.receipt.evidence.as_ref()?;
    if evidence.populated != Some(false) {
        return None;
    }
    // An actual retained attempt identity plus the terminal observation owns
    // this peak. The pre-provisioning placeholder is not a measured zero.
    if record.cgroup_inode.is_some()
        && record.unit_invocation.is_some()
        && evidence.peak_bytes > plan.empirical_ceiling_bytes
    {
        return Some(Failure::PeakExceeded);
    }
    // These are snapshots of the exact retained attempt and exclusively owned
    // processing parent, never of an unrelated finite host ancestor. Kernel
    // evidence can invalidate qualification even if a partial worker record
    // or a non-137 exit prevents the terminal classifier from reporting OOM.
    if record.cgroup_inode.is_some()
        && record.unit_invocation.is_some()
        && attempt_oom_killed(evidence)
        && owned_limit_pressure(evidence)
    {
        return Some(Failure::ProcessingOom);
    }
    (outcome == Outcome::AllocationFailed).then_some(Failure::AllocationFailed)
}

fn assess_qualification(registry: &mut Registry, record: &mut Record) -> Result<(), ErrorCode> {
    let failure = qualification_failure(record);
    let Some(captured) = record.film.as_mut() else {
        return Ok(());
    };
    if captured.grant.plan.qualified().is_none() {
        return Ok(());
    }
    if captured.qualification_failure.is_some() && captured.qualification_failure != failure {
        return Err(ErrorCode::Uncertain);
    }
    captured.qualification_failure = failure;
    let Some(reason) = failure else {
        return Ok(());
    };
    let invalidations = registry
        .invalidations
        .as_mut()
        .ok_or(ErrorCode::Uncertain)?;
    if let Some(existing) = invalidations.iter().find(|entry| {
        entry.envelope == captured.resource_model && entry.fixture_id == captured.grant.fixture.id
    }) {
        if existing.reason != reason
            || existing.incarnation != record.receipt.incarnation
            || existing.sequence != record.receipt.sequence
        {
            return Err(ErrorCode::Uncertain);
        }
    } else {
        if invalidations.len() >= crate::qualified::INVALIDATIONS {
            return Err(ErrorCode::Capacity);
        }
        invalidations.push(crate::qualified::Invalidation {
            envelope: captured.resource_model.clone(),
            fixture_id: captured.grant.fixture.id.clone(),
            reason,
            incarnation: record.receipt.incarnation.clone(),
            sequence: record.receipt.sequence,
        });
    }
    Ok(())
}

fn restore_qualifications(registry: &mut Registry) -> Result<(), ErrorCode> {
    // Called before any recovery manager effects, including for records whose
    // cleanup had already completed. Either the entire assessment persists or
    // startup retains the old registry and refuses admission.
    let mut next = registry.clone();
    for mut record in registry.records.values().cloned() {
        assess_qualification(&mut next, &mut record)?;
        next.records.insert(record.receipt.sequence, record);
    }
    *registry = next;
    Ok(())
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

/// The durable identity a claim records. It carries nothing else: a claim is
/// pure instance identity, so a claim whose start never completed stays
/// adoptable by the executor that holds it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    version: u8,
    root: String,
}

/// Create or adopt the instance claim file at `path` and take its exclusive
/// flock. Returns the retained descriptor and whether this process created
/// the claim. The identity semantics are shared by every processing
/// executor: version 1, an exact root match, one claim per instance, and a
/// busy refusal while another owner holds the flock. An existing claim whose
/// root has no registry stays quarantined: the flock proves exclusivity, not
/// completeness, so a lost or never-written registry is never adopted and the
/// previous ownership evidence survives.
pub(crate) fn hold_claim(
    path: &Path,
    namespace: &Path,
    root: &str,
) -> Result<(File, bool), ErrorCode> {
    let (mut file, fresh) = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => (file, true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| ErrorCode::Uncertain)?,
            false,
        ),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    // The production launcher runs as root, so this is the root-owned check;
    // focused tests exercise the same path as the invoking user.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
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
    if fresh {
        let bytes = serde_json::to_vec(&Claim {
            version: 1,
            root: root.to_owned(),
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
            || claim.root != root
            || !Path::new(root)
                .join("registry.json")
                .try_exists()
                .map_err(|_| ErrorCode::Uncertain)?
        {
            return Err(ErrorCode::Uncertain);
        }
    }
    Ok((file, fresh))
}

pub(crate) fn claim_instance(config: &Config) -> Result<(File, bool), ErrorCode> {
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
    hold_claim(
        &namespace.join(format!("{}.claim", config.instance)),
        namespace,
        &config.root,
    )
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
pub(crate) mod tests {
    use super::*;

    /// A scratch claim namespace outside the fixed host path, so the claim
    /// semantics run under any CI UID.
    fn claim_dir(tag: &str) -> std::path::PathBuf {
        let dir: std::path::PathBuf = std::env::temp_dir().join(format!(
            "slipstream-claim-{tag}-{}-{}",
            std::process::id(),
            random_id().unwrap()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)
            .unwrap();
        dir
    }

    /// Claims hold an exclusive descriptor, so assertions compare outcomes.
    fn claim_error(claim: Result<(File, bool), ErrorCode>) -> ErrorCode {
        claim.err().unwrap()
    }

    fn foreign_claim_bytes(root: &str) -> Vec<u8> {
        serde_json::to_vec(&Claim {
            version: 2,
            root: root.to_owned(),
        })
        .unwrap()
    }

    fn write_claim_file(path: &Path, bytes: &[u8], mode: u32) {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    #[test]
    fn a_claim_without_an_initialized_root_stays_quarantined_until_the_registry_exists() {
        let dir = claim_dir("quarantine");
        let path = dir.join("instance.claim");
        let root = dir.display().to_string();
        // A first start that was refused after claiming leaves exactly this
        // state: a released claim file and no registry.json.
        let (file, fresh) = hold_claim(&path, &dir, &root).unwrap();
        assert!(fresh);
        drop(file);
        assert!(!dir.join("registry.json").try_exists().unwrap());
        // The claim alone proves exclusivity, not completeness, so the
        // registry-less state is never adopted.
        assert_eq!(
            claim_error(hold_claim(&path, &dir, &root)),
            ErrorCode::Uncertain
        );
        // Once the root is initialized, the claim is adoptable again.
        fs::File::create(dir.join("registry.json")).unwrap();
        let (adopted, fresh) = hold_claim(&path, &dir, &root).unwrap();
        assert!(!fresh);
        // Adoption keeps the recorded identity; it never rewrites the claim.
        let claim: Claim = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(claim.version, 1);
        assert_eq!(claim.root, root);
        drop(adopted);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_held_claim_stays_busy() {
        let dir = claim_dir("busy");
        let path = dir.join("instance.claim");
        let held = hold_claim(&path, &dir, "/var/lib/slipstream-processing/a").unwrap();
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Busy
        );
        drop(held);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn foreign_root_and_foreign_version_claims_stay_refused() {
        let dir = claim_dir("foreign");
        let path = dir.join("instance.claim");
        // A claim written for a different root is never adoptable.
        drop(hold_claim(&path, &dir, "/var/lib/slipstream-processing/other").unwrap());
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_file(&path).unwrap();
        write_claim_file(
            &path,
            &foreign_claim_bytes("/var/lib/slipstream-processing/a"),
            0o600,
        );
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonconforming_claim_files_stay_refused() {
        let dir = claim_dir("nonconforming");
        let bytes = serde_json::to_vec(&Claim {
            version: 1,
            root: "/var/lib/slipstream-processing/a".to_owned(),
        })
        .unwrap();
        // A group-readable claim file fails the private-mode check.
        let path = dir.join("loose.claim");
        write_claim_file(&path, &bytes, 0o644);
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        // A symlinked claim path fails the no-follow check.
        let target = dir.join("target.claim");
        write_claim_file(&target, &bytes, 0o600);
        let link = dir.join("link.claim");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            claim_error(hold_claim(&link, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(dir).unwrap();
    }

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
            terminal_snapshot: None,
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
    fn descendant_oom_survives_a_false_or_missing_docker_flag_and_cancellation() {
        // Shape captured by the native leaf-pressure regression: the leaf
        // disappears, local counters stay zero, and Docker reports false.
        let mut value = evidence(137, false, Some(0), Some(0));
        value.attempt_after = Some(Events {
            oom: 1,
            oom_kill: 3,
            oom_group_kill: 1,
            ..Events::default()
        });
        for flag in [Some(false), None, Some(true)] {
            value.docker_oom_killed = flag;
            for requested in [None, Some(Outcome::Cancelled), Some(Outcome::Interrupted)] {
                assert_eq!(classify(&value, None, requested), Outcome::Oom);
            }
        }
    }

    #[test]
    fn fallback_oom_requires_terminal_kill_and_owned_limit_pressure() {
        let mut value = evidence(137, false, Some(0), Some(0));
        value.attempt_after.as_mut().unwrap().oom_kill = 3;
        // A host/global kill is insufficient even if an unrelated child of
        // the parent raised its hierarchical pressure counter.
        value.parent_before = Some(Events::default());
        value.parent_after = Some(events(3));
        assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
        value.parent_after.as_mut().unwrap().local_oom = 1;
        assert_eq!(classify(&value, None, None), Outcome::Oom);

        for missing_before in [true, false] {
            let mut missing = value.clone();
            if missing_before {
                missing.parent_before = None;
            } else {
                missing.parent_after = None;
            }
            assert_eq!(classify(&missing, None, None), Outcome::EngineFailed);
        }
        let mut regressed = value.clone();
        regressed.parent_before.as_mut().unwrap().local_oom = 2;
        assert_eq!(classify(&regressed, None, None), Outcome::EngineFailed);
        for before in [None, Some(events(3)), Some(events(4))] {
            let mut missing_kill = value.clone();
            missing_kill.attempt_before = before;
            assert_eq!(classify(&missing_kill, None, None), Outcome::EngineFailed);
        }
        let mut missing_kill = value.clone();
        missing_kill.attempt_after = None;
        assert_eq!(classify(&missing_kill, None, None), Outcome::EngineFailed);

        value.parent_before = None;
        value.parent_after = None;
        value.attempt_before.as_mut().unwrap().oom = 2;
        value.attempt_after.as_mut().unwrap().oom = 1;
        assert_eq!(classify(&value, None, None), Outcome::EngineFailed);
        value.attempt_after.as_mut().unwrap().oom = 3;
        assert_eq!(classify(&value, None, None), Outcome::Oom);
        for (code, worker, expected) in [
            (Some(0), Some(Outcome::Completed), Outcome::Completed),
            (
                Some(20),
                Some(Outcome::AllocationFailed),
                Outcome::AllocationFailed,
            ),
            (Some(21), Some(Outcome::StorageFull), Outcome::StorageFull),
            (Some(76), None, Outcome::Deadline),
            (Some(75), None, Outcome::EngineFailed),
            (None, None, Outcome::Unknown),
        ] {
            value.exit_code = code;
            assert_eq!(classify(&value, worker, None), expected);
        }
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
            invalidations: None,
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

    pub(crate) fn qualified_record(sequence: u64) -> Record {
        use crate::{film, qualified};
        let mut record = record(sequence, State::Settled);
        let mut grant = film::test_grant();
        let plan = qualified::Plan::calculate(
            &grant.fixture,
            &qualified::Case::Qualified {
                fixture_id: grant.fixture.id.clone(),
                empirical_ceiling_bytes: 1 << 30,
                safety_reserve_bytes: 1 << 20,
                evidence_sha256: "a".repeat(64),
            },
            &"b".repeat(64),
            &"c".repeat(64),
            8 << 30,
        )
        .unwrap();
        grant.version = 3;
        grant.kind = "film-qualified-grant".into();
        grant.launch_id = record.launch_id.clone();
        grant.bundle = record.receipt.bundle.clone();
        record.receipt.workload = Workload::Film(film::Workload {
            kind: "film-fixture".into(),
            fixture_id: grant.fixture.id.clone(),
        });
        record.receipt.limits = film::limits(8 << 30);
        record.unit_invocation = Some("d".repeat(32));
        record.cgroup_inode = Some(1);
        record.film = Some(film::Captured {
            catalogue: "e".repeat(64),
            resource_model: plan.envelope_sha256.clone(),
            grant: grant.map_plan(|_| film::ExecutionPlan::Qualified(plan)),
            phase: film::Phase::ExecutionFinished,
            stage_release_intent: true,
            engine_release_intent: true,
            grant_file: None,
            snapshot: None,
            result_file: None,
            result: None,
            detail: None,
            qualification_failure: None,
            qualification_observation_valid: Some(true),
        });
        record
    }

    fn qualified_registry(record: Record) -> Registry {
        Registry {
            version: 3,
            invalidations: Some(Vec::new()),
            watermark: record.receipt.sequence,
            records: BTreeMap::from([(record.receipt.sequence, record)]),
            ..registry()
        }
    }

    #[test]
    fn full_invalidation_capacity_refuses_new_plans_without_redefining_capability() {
        use crate::{film, qualified};
        let record = qualified_record(1);
        let captured = record.film.as_ref().unwrap();
        let grant = &captured.grant;
        let documents = qualified::Documents {
            catalogue: film::Catalogue {
                version: 1,
                numerical_bundle: grant.numerical_bundle.clone(),
                recipe: grant.recipe.clone(),
                procedure: grant.procedure.clone(),
                reference_image: record.image_id.clone(),
                input_icc_sha256: grant.input_icc_sha256.clone(),
                output_icc_sha256: grant.output_icc_sha256.clone(),
                fixtures: vec![grant.fixture.clone()],
            },
            envelope: qualified::Envelope {
                version: 1,
                formula: qualified::FORMULA.into(),
                inventory: qualified::INVENTORY.into(),
                catalogue_sha256: captured.catalogue.clone(),
                image: record.image_id.clone(),
                launcher_sha256: "c".repeat(64),
                environment: qualified::Environment {
                    machine: "x86_64".into(),
                    kernel_release: "test-kernel".into(),
                    page_bytes: 4096,
                    cpu_sha256: "d".repeat(64),
                    manager_sha256: "e".repeat(64),
                },
                cases: vec![qualified::Case::Qualified {
                    fixture_id: grant.fixture.id.clone(),
                    empirical_ceiling_bytes: 1 << 30,
                    safety_reserve_bytes: 1 << 20,
                    evidence_sha256: "f".repeat(64),
                }],
            },
        };
        let mut failures: Vec<_> = (1..=256)
            .map(|sequence| qualified::Invalidation {
                envelope: format!("{sequence:064x}"),
                fixture_id: grant.fixture.id.clone(),
                reason: qualified::QualificationFailure::PeakExceeded,
                incarnation: record.receipt.incarnation.clone(),
                sequence,
            })
            .collect();
        let envelope = &captured.resource_model;
        assert_eq!(
            qualified_availability(true, &documents.envelope.cases, envelope, &failures),
            qualified::Availability::Available
        );
        assert_eq!(
            plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
            Err(ErrorCode::Capacity)
        );
        failures[0].envelope = envelope.clone();
        assert_eq!(
            qualified_availability(true, &documents.envelope.cases, envelope, &failures),
            qualified::Availability::Unqualified
        );
        assert_eq!(
            plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
            Err(ErrorCode::Capacity)
        );
        assert_eq!(
            qualified_availability(false, &documents.envelope.cases, envelope, &failures),
            qualified::Availability::Blocked
        );
        failures.pop();
        assert_eq!(
            plan_qualified_start(&documents, &failures, &grant.fixture.id, envelope, 8 << 30),
            Err(ErrorCode::UnqualifiedEnvelope)
        );
        assert!(
            plan_qualified_start(
                &documents,
                &failures,
                &grant.fixture.id,
                &"a".repeat(64),
                8 << 30
            )
            .is_ok()
        );
        assert_eq!(failures.len(), 255);
    }

    #[test]
    fn qualification_contradictions_preserve_outcome_and_require_owned_observations() {
        use crate::qualified::QualificationFailure as Failure;
        let mut record = qualified_record(1);
        let ceiling = record
            .film
            .as_ref()
            .unwrap()
            .grant
            .plan
            .qualified()
            .unwrap()
            .empirical_ceiling_bytes;
        record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling;
        assert_eq!(qualification_failure(&record), None);
        record.receipt.evidence.as_mut().unwrap().peak_bytes += 1;
        assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
        assert_eq!(record.receipt.outcome, Some(Outcome::Completed));
        record.receipt.outcome = Some(Outcome::Cancelled);
        assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
        record
            .film
            .as_mut()
            .unwrap()
            .qualification_observation_valid = Some(false);
        assert_eq!(qualification_failure(&record), None);
        record
            .film
            .as_mut()
            .unwrap()
            .qualification_observation_valid = Some(true);
        record.cgroup_inode = None;
        assert_eq!(qualification_failure(&record), None);
        record.cgroup_inode = Some(1);
        let evidence = record.receipt.evidence.as_mut().unwrap();
        evidence.peak_bytes = ceiling;
        evidence.attempt_after.as_mut().unwrap().oom_kill = 1;
        evidence.docker_oom_killed = Some(true);
        assert_eq!(
            qualification_failure(&record),
            None,
            "a kill can come from outside the owned boundary"
        );
        // Owned leaf pressure is hierarchical even when local counters and
        // Docker's flag remain unchanged. A partial worker outcome does not
        // erase this independently established qualification contradiction.
        let evidence = record.receipt.evidence.as_mut().unwrap();
        evidence.docker_oom_killed = Some(false);
        evidence.attempt_after.as_mut().unwrap().oom = 1;
        assert_eq!(qualification_failure(&record), Some(Failure::ProcessingOom));
        assert_eq!(record.receipt.outcome, Some(Outcome::Cancelled));
        record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling + 1;
        assert_eq!(qualification_failure(&record), Some(Failure::PeakExceeded));
        record.receipt.evidence.as_mut().unwrap().peak_bytes = ceiling;
        for populated in [Some(true), None] {
            record.receipt.evidence.as_mut().unwrap().populated = populated;
            assert_eq!(qualification_failure(&record), None);
        }
        record.receipt.evidence.as_mut().unwrap().populated = Some(false);
        record.unit_invocation = None;
        assert_eq!(qualification_failure(&record), None);
        record.unit_invocation = Some("a".repeat(32));
        record.cgroup_inode = None;
        assert_eq!(qualification_failure(&record), None);
        record.cgroup_inode = Some(1);
        record
            .film
            .as_mut()
            .unwrap()
            .qualification_observation_valid = Some(false);
        assert_eq!(qualification_failure(&record), None);
        record
            .film
            .as_mut()
            .unwrap()
            .qualification_observation_valid = Some(true);

        // An exact parent limit can kill this attempt without increasing the
        // attempt's own oom counter. Parent kill or pressure alone cannot
        // prove that this attempt was killed by an owned limit.
        let evidence = record.receipt.evidence.as_mut().unwrap();
        evidence.attempt_after.as_mut().unwrap().oom = 0;
        evidence.parent_before = Some(Events::default());
        evidence.parent_after = Some(Events {
            oom: 1,
            oom_kill: 1,
            local_oom: 1,
            ..Events::default()
        });
        assert_eq!(qualification_failure(&record), Some(Failure::ProcessingOom));
        let proven_parent = record.clone();
        for kind in 0..7 {
            let mut invalid = proven_parent.clone();
            let evidence = invalid.receipt.evidence.as_mut().unwrap();
            match kind {
                0 => evidence.attempt_before = None,
                1 => evidence.attempt_after = None,
                2 => evidence.attempt_after.as_mut().unwrap().oom_kill = 0,
                3 => evidence.attempt_before.as_mut().unwrap().oom_kill = 2,
                4 => evidence.parent_before = None,
                5 => evidence.parent_after.as_mut().unwrap().local_oom = 0,
                6 => evidence.parent_before.as_mut().unwrap().local_oom = 2,
                _ => unreachable!(),
            }
            assert_eq!(qualification_failure(&invalid), None, "case {kind}");
        }
        let evidence = record.receipt.evidence.as_mut().unwrap();
        evidence.attempt_after = Some(Events::default());
        evidence.parent_after = Some(Events::default());
        record.receipt.outcome = Some(Outcome::AllocationFailed);
        assert_eq!(
            qualification_failure(&record),
            Some(Failure::AllocationFailed)
        );
        for outcome in [
            Outcome::Cancelled,
            Outcome::StorageFull,
            Outcome::EngineFailed,
            Outcome::Interrupted,
        ] {
            record.receipt.outcome = Some(outcome);
            assert_eq!(qualification_failure(&record), None);
        }
    }

    #[test]
    fn recovered_descendant_oom_withdraws_the_exact_qualified_case() {
        let mut record = qualified_record(1);
        let evidence = record.receipt.evidence.as_mut().unwrap();
        evidence.exit_code = Some(137);
        evidence.docker_oom_killed = Some(false);
        evidence.attempt_after = Some(Events {
            oom: 1,
            oom_kill: 3,
            oom_group_kill: 1,
            ..Events::default()
        });
        record.receipt.outcome = Some(classify(evidence, None, None));
        let fixture = record.film.as_ref().unwrap().grant.fixture.id.clone();
        let envelope = record.film.as_ref().unwrap().resource_model.clone();
        let mut registry = qualified_registry(record);
        restore_qualifications(&mut registry).unwrap();
        let invalidations = registry.invalidations.as_ref().unwrap();
        assert_eq!(invalidations.len(), 1);
        assert_eq!(invalidations[0].fixture_id, fixture);
        assert_eq!(invalidations[0].envelope, envelope);
        assert_eq!(
            invalidations[0].reason,
            crate::qualified::QualificationFailure::ProcessingOom
        );
        assert_eq!(registry.records[&1].receipt.outcome, Some(Outcome::Oom));
        let bytes = serde_json::to_vec(&registry).unwrap();
        let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
        restore_qualifications(&mut recovered).unwrap();
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
    }

    #[test]
    fn recovered_contradiction_is_atomic_and_survives_receipt_expiry_and_restart() {
        let mut record = qualified_record(1);
        record.receipt.outcome = Some(Outcome::AllocationFailed);
        let mut registry = qualified_registry(record);
        restore_qualifications(&mut registry).unwrap();
        let original = serde_json::to_vec(&registry).unwrap();
        let mut recovered: Registry = serde_json::from_slice(&original).unwrap();
        restore_qualifications(&mut recovered).unwrap();
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), original);
        let record = recovered.records.get(&1).unwrap();
        assert_eq!(record.receipt.outcome, Some(Outcome::AllocationFailed));
        assert_eq!(
            record.film.as_ref().unwrap().qualification_failure,
            Some(crate::qualified::QualificationFailure::AllocationFailed)
        );
        let tombstone = recovered.invalidations.as_ref().unwrap()[0].clone();
        expire(&mut recovered, 2000, 1).unwrap();
        assert!(recovered.records.is_empty());
        assert_eq!(recovered.invalidations, Some(vec![tombstone]));
        let bytes = serde_json::to_vec(&recovered).unwrap();
        let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
        restore_qualifications(&mut recovered).unwrap();
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
    }

    #[test]
    fn invalidated_observation_context_survives_restart_without_a_false_model_failure() {
        let mut record = qualified_record(1);
        record.receipt.outcome = Some(Outcome::Interrupted);
        record.receipt.evidence.as_mut().unwrap().peak_bytes = u64::MAX;
        record
            .film
            .as_mut()
            .unwrap()
            .qualification_observation_valid = Some(false);
        let registry = qualified_registry(record);
        let mut recovered: Registry =
            serde_json::from_slice(&serde_json::to_vec(&registry).unwrap()).unwrap();
        restore_qualifications(&mut recovered).unwrap();
        assert!(recovered.invalidations.unwrap().is_empty());
        let captured = recovered.records[&1].film.as_ref().unwrap();
        assert_eq!(captured.qualification_observation_valid, Some(false));
        assert_eq!(captured.qualification_failure, None);
    }

    #[test]
    fn missing_or_full_invalidation_state_cannot_be_reinitialized_during_recovery() {
        let mut record = qualified_record(256);
        record.receipt.outcome = Some(Outcome::AllocationFailed);
        let mut registry = qualified_registry(record);
        registry.invalidations = None;
        assert_eq!(
            restore_qualifications(&mut registry),
            Err(ErrorCode::Uncertain)
        );
        registry.invalidations = Some(
            (1..=256)
                .map(|sequence| crate::qualified::Invalidation {
                    envelope: format!("{sequence:064x}"),
                    fixture_id: "a".repeat(32),
                    reason: crate::qualified::QualificationFailure::PeakExceeded,
                    incarnation: registry.incarnation.clone(),
                    sequence,
                })
                .collect(),
        );
        let original = serde_json::to_vec(&registry).unwrap();
        assert_eq!(
            restore_qualifications(&mut registry),
            Err(ErrorCode::Capacity)
        );
        assert_eq!(serde_json::to_vec(&registry).unwrap(), original);
        registry.invalidations.as_mut().unwrap().pop();
        restore_qualifications(&mut registry).unwrap();
        assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
        restore_qualifications(&mut registry).unwrap();
        assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
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
        for field in ["policy", "bundle", "image", "invocation"] {
            let mut changed = registry();
            let record = changed.records.get_mut(&1).unwrap();
            match field {
                "policy" => record.receipt.policy.push('a'),
                "bundle" => record.receipt.bundle = "z".repeat(64),
                "image" => record.image_id.push('a'),
                "invocation" => record.unit_invocation = Some("f".repeat(33)),
                _ => unreachable!(),
            }
            assert_eq!(
                validate_registry(&changed, &config),
                Err(ErrorCode::Uncertain)
            );
        }
    }
    fn maximal_film_registry() -> Registry {
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
                grant: grant
                    .clone()
                    .map_plan(crate::film::ExecutionPlan::Measurement),
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
                qualification_failure: None,
                qualification_observation_valid: None,
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
            let memory_peak_raw = format!("{}\n", u64::MAX);
            let memory_max_raw = format!("{}\n", record.receipt.limits.memory_bytes);
            let memory_swap_current_raw = "0\n".to_owned();
            let memory_swap_max_raw = "0\n".to_owned();
            let mut memory_events_raw = format!(
                "oom {}\noom_kill {}\noom_group_kill {}\n",
                u64::MAX,
                u64::MAX,
                u64::MAX
            );
            let mut memory_events_local_raw = memory_events_raw.clone();
            let mut padding_index = 0_u64;
            loop {
                let total = memory_peak_raw.len()
                    + memory_max_raw.len()
                    + memory_swap_current_raw.len()
                    + memory_swap_max_raw.len()
                    + memory_events_raw.len()
                    + memory_events_local_raw.len();
                let line = format!("future_{padding_index} 0\n");
                if total + line.len() > TERMINAL_SNAPSHOT_BYTES {
                    break;
                }
                if padding_index.is_multiple_of(2) {
                    memory_events_raw.push_str(&line);
                } else {
                    memory_events_local_raw.push_str(&line);
                }
                padding_index += 1;
            }
            record.receipt.evidence = Some(Evidence {
                peak_bytes: u64::MAX,
                exit_code: Some(0),
                docker_oom_killed: Some(false),
                attempt_before: Some(events.clone()),
                attempt_after: Some(events.clone()),
                parent_before: Some(events.clone()),
                parent_after: Some(events),
                populated: Some(false),
                terminal_snapshot: Some(TerminalSnapshot {
                    cgroup_path: format!(
                        "/sys/fs/cgroup/slipstreamprocessing0.slice/{}",
                        record.unit()
                    ),
                    cgroup_inode: u64::MAX,
                    unit_invocation: "6".repeat(32),
                    launch_id: record.launch_id.clone(),
                    container_id: record
                        .receipt
                        .runtime
                        .as_ref()
                        .unwrap()
                        .container_id
                        .clone()
                        .unwrap(),
                    attempt_unit: record.unit().to_owned(),
                    incarnation: registry.incarnation.clone(),
                    sequence: record.receipt.sequence,
                    memory_peak_raw,
                    memory_max_raw,
                    memory_swap_current_raw,
                    memory_swap_max_raw,
                    memory_events_raw,
                    memory_events_local_raw,
                    io_stat_raw: None,
                }),
            });
        }
        registry
    }

    #[test]
    fn maximal_film_receipts_fit_all_fixed_protocol_and_journal_bounds() {
        use crate::film;
        let registry = maximal_film_registry();
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

    #[test]
    fn maximal_qualified_internal_records_reserve_terminal_capacity_with_all_invalidations() {
        use crate::{film, qualified};
        let mut registry = maximal_film_registry();
        registry.version = 3;
        registry.invalidations = Some(Vec::new());
        for record in registry.records.values_mut() {
            let captured = record.film.as_mut().unwrap();
            let grant = &mut captured.grant;
            grant.version = 3;
            grant.kind = "film-qualified-grant".into();
            grant.bundle = record.receipt.bundle.clone();
            captured.resource_model = format!("{:064x}", record.receipt.sequence);
            let plan = qualified::Plan::calculate(
                &grant.fixture,
                &qualified::Case::Qualified {
                    fixture_id: grant.fixture.id.clone(),
                    empirical_ceiling_bytes: 13 << 30,
                    safety_reserve_bytes: (32 << 30)
                        - film::STORAGE
                        - grant.fixture.source_bytes()
                        - (13 << 30),
                    evidence_sha256: "f".repeat(64),
                },
                &captured.resource_model,
                &"a".repeat(64),
                32 << 30,
            )
            .unwrap();
            assert_eq!(plan.required_bytes, 32 << 30);
            assert_eq!(plan.missing.len(), 58);
            grant.plan = film::ExecutionPlan::Qualified(plan);
            grant.validate().unwrap();
            if let Some(film::WorkerResult::Success(result)) = &mut captured.result {
                result.plan_sha256 = film::hash(&grant.plan).unwrap();
                result.artifact.width = grant.fixture.width;
                result.artifact.height = grant.fixture.height;
            }
            captured.result.as_ref().unwrap().validate(grant).unwrap();
            captured.qualification_observation_valid = Some(true);
            record.unit_invocation = Some("f".repeat(32));
            record.cgroup_inode = Some(u64::MAX);
            record.mount_id = Some(u64::MAX);
            record.termination_reason = Some(Outcome::Interrupted);
            record.settled_at_unix_ms = Some(u64::MAX);
        }
        restore_qualifications(&mut registry).unwrap();
        assert_eq!(registry.invalidations.as_ref().unwrap().len(), 256);
        let config = Config {
            version: 3,
            mode: "film-qualified-fixtures".into(),
            instance: registry.instance.clone(),
            root: "/test".into(),
            socket: "/test.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "4".repeat(64)),
            memory_bytes: 32 << 30,
            receipt_retention_seconds: 604800,
        };
        validate_registry(&registry, &config).unwrap();
        let bytes = serde_json::to_vec(&registry).unwrap();
        let mut recovered: Registry = serde_json::from_slice(&bytes).unwrap();
        validate_registry(&recovered, &config).unwrap();
        restore_qualifications(&mut recovered).unwrap();
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
        let record = &registry.records[&u64::MAX];
        let grant = film::canonical(&record.film.as_ref().unwrap().grant).unwrap();
        let response = serde_json::to_vec(&Response::Result {
            version: 3,
            result: Box::new(record.result_body()),
        })
        .unwrap();
        let _: Response = serde_json::from_slice(&response).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(
            wire["result"]["receipt"]["qualification_failure"],
            "peak-exceeded"
        );
        assert!(wire["result"]["receipt"].get("resource_model").is_none());
        assert!(
            bytes.len() < 4 * 1024 * 1024,
            "registry bytes={}",
            bytes.len()
        );
        assert!(grant.len() <= film::FRAME);
        assert!(response.len() <= RESPONSE_BYTES);
        // Each admitted record is structurally bounded by this fully populated
        // capture, including its future terminal result and tombstone. There is
        // no reserve that relies on the much smaller accepted-state record.
        println!(
            "maximal qualified bounds: registry256+invalidations256={} grant={} response={}",
            bytes.len(),
            grant.len(),
            response.len()
        );
    }
}
