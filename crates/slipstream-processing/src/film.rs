//! Closed Film measurement contracts. Image bytes never enter this module.
use crate::protocol::{ErrorCode, Limits, Outcome, digest, hex};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub const STORAGE: u64 = 4 * 1024 * 1024 * 1024;
pub const FRAME: usize = 16 * 1024;
pub const PROFILE: &str = "slipstream-film-measurement-v1";
pub const PROCEDURE: &str = "film-once-empty-cache-v1";
pub const FORMULA: &str = "film-live-storage-v1";
pub const GAMUT_ALLOWANCE: u64 = 603_979_776;
pub const JPEG_ALLOWANCE: u64 = 17_825_792;
pub const WORKER: &str = "/usr/local/bin/slipstream-processing-film-worker";
pub type Result<T> = std::result::Result<T, ErrorCode>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub mode: String,
    pub instance: String,
    pub root: String,
    pub socket: String,
    pub peer_uid: u32,
    pub image: String,
    pub memory_bytes: u64,
    pub receipt_retention_seconds: u64,
    pub catalogue_sha256: String,
    pub resource_model_sha256: String,
}
impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Self = parse(bytes, FRAME)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        let authority = crate::protocol::Config {
            version: 1,
            mode: "qualification".into(),
            instance: self.instance.clone(),
            root: self.root.clone(),
            socket: self.socket.clone(),
            peer_uid: self.peer_uid,
            image: format!("sha256:{}", "0".repeat(64)),
            memory_bytes: 128 * 1024 * 1024,
            receipt_retention_seconds: self.receipt_retention_seconds,
        };
        authority.validate()?;
        if !image(&self.image)
            || self.root.chars().count() < 2
            || self.root.chars().count() > 4096
            || self.socket.chars().count() < 2
            || self.socket.chars().count() > 4096
            || self.version != 2
            || self.mode != "film-measurement"
            || self.peer_uid != 0
            || ![8, 12, 16, 24, 32]
                .map(|n| n * 1024 * 1024 * 1024)
                .contains(&self.memory_bytes)
            || !hex(&self.catalogue_sha256, 64)
            || !hex(&self.resource_model_sha256, 64)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
    pub fn authority(&self) -> crate::protocol::Config {
        crate::protocol::Config {
            version: 2,
            mode: self.mode.clone(),
            instance: self.instance.clone(),
            root: self.root.clone(),
            socket: self.socket.clone(),
            peer_uid: 0,
            image: self.image.clone(),
            memory_bytes: self.memory_bytes,
            receipt_retention_seconds: self.receipt_retention_seconds,
        }
    }
    pub fn policy(&self, image: &str) -> Result<String> {
        hash(&serde_json::json!({"config": self, "resolved_image": image}))
    }
    pub fn limits(&self) -> Limits {
        limits(self.memory_bytes)
    }
}
pub fn limits(memory_bytes: u64) -> Limits {
    Limits {
        memory_bytes,
        swap_bytes: 0,
        cpu_quota_us: 400_000,
        cpu_period_us: 100_000,
        tasks: 256,
        storage_bytes: STORAGE,
        storage_inodes: 4096,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub version: u8,
    pub numerical_bundle: String,
    pub recipe: String,
    pub procedure: String,
    pub reference_image: String,
    pub input_icc_sha256: String,
    pub output_icc_sha256: String,
    pub fixtures: Vec<Fixture>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub width: u64,
    pub height: u64,
    pub source: Source,
    pub reference: Reference,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    DevelopmentTiff {
        bytes: u64,
        sha256: String,
    },
    SyntheticRgb {
        generator: String,
        pattern: Pattern,
        seed: u64,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    Dark,
    Bright,
    Red,
    Green,
    Blue,
    Gradient,
    Noise,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub input_pixels_sha256: String,
    pub film_pixels_sha256: String,
    pub jpeg_sha256: String,
    pub jpeg_bytes: u64,
    pub evidence_sha256: String,
}
impl Fixture {
    pub fn validate(&self) -> Result<()> {
        if !hex(&self.id, 32)
            || self.reference.jpeg_bytes == 0
            || self.reference.jpeg_bytes > 512 * 1024 * 1024
            || [
                &self.reference.input_pixels_sha256,
                &self.reference.film_pixels_sha256,
                &self.reference.jpeg_sha256,
                &self.reference.evidence_sha256,
            ]
            .iter()
            .any(|s| !hex(s, 64))
        {
            return Err(ErrorCode::InvalidRequest);
        }
        match &self.source {
            Source::DevelopmentTiff { bytes, sha256 }
                if *bytes == 0 || *bytes > 2 * 1024 * 1024 * 1024 || !hex(sha256, 64) =>
            {
                return Err(ErrorCode::InvalidRequest);
            }
            Source::SyntheticRgb {
                generator,
                pattern,
                seed,
            } if generator != "linear-rgb-f32-v1"
                || *seed > u32::MAX as u64
                || (*pattern != Pattern::Noise && *seed != 0) =>
            {
                return Err(ErrorCode::InvalidRequest);
            }
            _ => {}
        }
        if !(1..=9568).contains(&self.width) || !(1..=9568).contains(&self.height) {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
    pub fn source_bytes(&self) -> u64 {
        match self.source {
            Source::DevelopmentTiff { bytes, .. } => bytes,
            _ => 0,
        }
    }
}
impl Catalogue {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.procedure != PROCEDURE
            || !(1..=16).contains(&self.fixtures.len())
            || [
                &self.numerical_bundle,
                &self.recipe,
                &self.input_icc_sha256,
                &self.output_icc_sha256,
            ]
            .iter()
            .any(|s| !hex(s, 64))
            || !image(&self.reference_image)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut ids = BTreeSet::new();
        for fixture in &self.fixtures {
            fixture.validate()?;
            if !ids.insert(&fixture.id) {
                return Err(ErrorCode::InvalidRequest);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Staging,
    Decode,
    Initialization,
    Preprocess,
    Exposure,
    Development,
    Printing,
    Scan,
    Gamut,
    Cctf,
    Jpeg,
    Validation,
}
pub const STAGES: [Stage; 12] = [
    Stage::Staging,
    Stage::Decode,
    Stage::Initialization,
    Stage::Preprocess,
    Stage::Exposure,
    Stage::Development,
    Stage::Printing,
    Stage::Scan,
    Stage::Gamut,
    Stage::Cctf,
    Stage::Jpeg,
    Stage::Validation,
];
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Term {
    OwnedArrays,
    Runtime,
    Native,
    AllocatorRetention,
    Kernel,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Bound {
    Unknown,
    Qualified { bytes: u64, evidence_sha256: String },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StageBounds {
    pub stage: Stage,
    pub runtime: Bound,
    pub native: Bound,
    pub allocator_retention: Bound,
    pub kernel: Bound,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelCase {
    pub fixture_id: String,
    pub stages: Vec<StageBounds>,
}
impl ModelCase {
    fn ordered_stages(&self) -> Result<Vec<&StageBounds>> {
        let mut stages: Vec<_> = self.stages.iter().collect();
        stages.sort_unstable_by_key(|bounds| bounds.stage);
        if stages.iter().map(|bounds| bounds.stage).ne(STAGES) {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(stages)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceModel {
    pub version: u8,
    pub catalogue_sha256: String,
    pub numerical_bundle: String,
    pub formula: String,
    pub cases: Vec<ModelCase>,
}
impl ResourceModel {
    fn validate(&self, catalogue: &Catalogue, catalogue_hash: &str) -> Result<()> {
        if self.version != 1
            || self.catalogue_sha256 != catalogue_hash
            || self.numerical_bundle != catalogue.numerical_bundle
            || self.formula != FORMULA
            || self.cases.len() != catalogue.fixtures.len()
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            if !ids.insert(&case.fixture_id)
                || !catalogue.fixtures.iter().any(|f| f.id == case.fixture_id)
            {
                return Err(ErrorCode::InvalidRequest);
            }
            for s in case.ordered_stages()? {
                for term in [&s.runtime, &s.native, &s.allocator_retention, &s.kernel] {
                    if let Bound::Qualified {
                        evidence_sha256, ..
                    } = term
                        && !hex(evidence_sha256, 64)
                    {
                        return Err(ErrorCode::InvalidRequest);
                    }
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Documents {
    pub catalogue: Catalogue,
    pub model: ResourceModel,
}
impl Documents {
    pub fn load(config: &Config) -> Result<Self> {
        let catalogue: Catalogue = metadata_file(
            &Path::new(&config.root).join("catalogue.json"),
            32768,
            &config.catalogue_sha256,
        )?;
        catalogue.validate()?;
        let model: ResourceModel = metadata_file(
            &Path::new(&config.root).join("resource-model.json"),
            131072,
            &config.resource_model_sha256,
        )?;
        model.validate(&catalogue, &config.catalogue_sha256)?;
        Ok(Self { catalogue, model })
    }
    pub fn plan(&self, id: &str, memory: u64) -> Result<(Fixture, Plan)> {
        let fixture = self
            .catalogue
            .fixtures
            .iter()
            .find(|f| f.id == id)
            .ok_or(ErrorCode::UnknownFixture)?;
        let case = self
            .model
            .cases
            .iter()
            .find(|c| c.fixture_id == id)
            .ok_or(ErrorCode::IncompatibleResourceModel)?;
        Ok((fixture.clone(), plan(fixture, case, memory)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalPlan {
    pub model: String,
    pub allowance_bytes: u64,
    pub scratch_bytes: u64,
    pub batch_pixels: u64,
    pub destination_bytes: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MissingTerm {
    pub stage: Stage,
    pub term: Term,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Prediction {
    Unqualified {
        known_required_bytes: u64,
        known_terms_exceed_limit: bool,
        missing: Vec<MissingTerm>,
    },
    FitsModel {
        required_bytes: u64,
    },
    ExceedsModel {
        required_bytes: u64,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub formula: String,
    pub width: u64,
    pub height: u64,
    pub source_cache_bytes: u64,
    pub storage_reserve_bytes: u64,
    pub prediction: Prediction,
    pub gamut: LocalPlan,
    pub cctf: LocalPlan,
    pub jpeg: LocalPlan,
}

pub fn geometry(width: u64, height: u64) -> Result<u64> {
    let count = width
        .checked_mul(height)
        .ok_or(ErrorCode::UnsupportedFixture)?;
    if !(1..=9568).contains(&width)
        || !(1..=9568).contains(&height)
        || width.max(height).checked_mul(3).is_none_or(|n| n > 175000)
        || count
            .checked_mul(12)
            .is_none_or(|n| n > 2 * 1024 * 1024 * 1024)
        || count.checked_mul(24).is_none_or(|n| n > i64::MAX as u64)
    {
        return Err(ErrorCode::UnsupportedFixture);
    }
    Ok(count)
}
pub fn local_plan(model: &str, pixels: u64, width: u64, allowance: u64) -> Result<LocalPlan> {
    let (fixed, per, max_allowance, destination) = match model {
        "cam16ucs-srgb-f64-v1" => (67_108_864u64, 2048u64, GAMUT_ALLOWANCE, 24u64),
        "srgb-cctf-f64-v1" => (4_194_304, 256, GAMUT_ALLOWANCE, 24),
        "jpeg-uint8-rows-v1" => (1_048_576, 64, JPEG_ALLOWANCE, 0),
        _ => return Err(ErrorCode::UnsupportedFixture),
    };
    if pixels == 0
        || pixels > i64::MAX as u64
        || allowance > max_allowance
        || allowance < fixed + per
        || width == 0
    {
        return Err(ErrorCode::UnsupportedFixture);
    }
    let destination_bytes = pixels
        .checked_mul(destination)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or(ErrorCode::UnsupportedFixture)?;
    let mut batch = ((allowance - fixed) / per).min(262144).min(pixels);
    if destination == 0 {
        if !pixels.is_multiple_of(width) {
            return Err(ErrorCode::UnsupportedFixture);
        }
        batch = batch / width * width;
    }
    if batch == 0 {
        return Err(ErrorCode::UnsupportedFixture);
    }
    Ok(LocalPlan {
        model: model.into(),
        allowance_bytes: allowance,
        scratch_bytes: fixed + batch * per,
        batch_pixels: batch,
        destination_bytes,
    })
}
pub fn plan(fixture: &Fixture, case: &ModelCase, memory: u64) -> Result<Plan> {
    let n = geometry(fixture.width, fixture.height)?;
    let gamut = local_plan("cam16ucs-srgb-f64-v1", n, fixture.width, GAMUT_ALLOWANCE)?;
    let cctf = local_plan("srgb-cctf-f64-v1", n, fixture.width, GAMUT_ALLOWANCE)?;
    let jpeg = local_plan("jpeg-uint8-rows-v1", n, fixture.width, JPEG_ALLOWANCE)?;
    let mut known_required = 0;
    let mut missing = Vec::new();
    for bounds in case.ordered_stages()? {
        let mut known = STORAGE
            .checked_add(fixture.source_bytes())
            .ok_or(ErrorCode::UnsupportedFixture)?;
        // Only inventories whose complete backing-allocation union is established are numeric.
        // Every simulation inventory remains explicitly unknown pending qualification.
        let (arrays, scratch) = match bounds.stage {
            Stage::Staging => (Some(0), 65536),
            Stage::Validation => (Some(0), 65536),
            Stage::Gamut => (None, gamut.scratch_bytes),
            Stage::Cctf => (None, cctf.scratch_bytes),
            Stage::Jpeg => (None, jpeg.scratch_bytes),
            _ => (None, 0),
        };
        known = known
            .checked_add(scratch)
            .ok_or(ErrorCode::UnsupportedFixture)?;
        if let Some(bytes) = arrays {
            known = known
                .checked_add(bytes)
                .ok_or(ErrorCode::UnsupportedFixture)?;
        } else {
            missing.push(MissingTerm {
                stage: bounds.stage,
                term: Term::OwnedArrays,
            });
        }
        for (term, bound) in [
            (Term::Runtime, &bounds.runtime),
            (Term::Native, &bounds.native),
            (Term::AllocatorRetention, &bounds.allocator_retention),
            (Term::Kernel, &bounds.kernel),
        ] {
            match bound {
                Bound::Unknown => missing.push(MissingTerm {
                    stage: bounds.stage,
                    term,
                }),
                Bound::Qualified { bytes, .. } => {
                    known = known
                        .checked_add(*bytes)
                        .ok_or(ErrorCode::UnsupportedFixture)?
                }
            }
        }
        known_required = known_required.max(known);
    }
    let prediction = if missing.is_empty() {
        if known_required <= memory {
            Prediction::FitsModel {
                required_bytes: known_required,
            }
        } else {
            Prediction::ExceedsModel {
                required_bytes: known_required,
            }
        }
    } else {
        Prediction::Unqualified {
            known_required_bytes: known_required,
            known_terms_exceed_limit: known_required > memory,
            missing,
        }
    };
    Ok(Plan {
        formula: FORMULA.into(),
        width: fixture.width,
        height: fixture.height,
        source_cache_bytes: fixture.source_bytes(),
        storage_reserve_bytes: STORAGE,
        prediction,
        gamut,
        cctf,
        jpeg,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub kind: String,
    pub fixture_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Reconcile {
        version: u8,
        instance: String,
    },
    Start {
        version: u8,
        instance: String,
        incarnation: String,
        sequence: u64,
        policy: String,
        bundle: String,
        catalogue: String,
        resource_model: String,
        workload: Workload,
    },
    Inspect {
        version: u8,
        instance: String,
        incarnation: String,
        sequence: u64,
    },
    Cancel {
        version: u8,
        instance: String,
        incarnation: String,
        sequence: u64,
    },
}
impl Request {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let request: Self = parse(bytes, FRAME)?;
        let (version, instance) = match &request {
            Self::Reconcile { version, instance }
            | Self::Start {
                version, instance, ..
            }
            | Self::Inspect {
                version, instance, ..
            }
            | Self::Cancel {
                version, instance, ..
            } => (*version, instance),
        };
        if version != 2 || !hex(instance, 32) {
            return Err(ErrorCode::InvalidRequest);
        }
        match &request {
            Self::Start {
                incarnation,
                sequence,
                policy,
                bundle,
                catalogue,
                resource_model,
                workload,
                ..
            } => {
                if !hex(incarnation, 32)
                    || *sequence == 0
                    || [policy, bundle, catalogue, resource_model]
                        .iter()
                        .any(|s| !hex(s, 64))
                    || workload.kind != "film-fixture"
                    || !hex(&workload.fixture_id, 32)
                {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
            Self::Inspect {
                incarnation,
                sequence,
                ..
            }
            | Self::Cancel {
                incarnation,
                sequence,
                ..
            } if !hex(incarnation, 32) || *sequence == 0 => return Err(ErrorCode::InvalidRequest),
            _ => {}
        }
        Ok(request)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Preparing,
    Staging,
    Sealed,
    Engine,
    Validating,
    ExecutionFinished,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Detail {
    SourceMismatch,
    UnsupportedInput,
    PlanRejected,
    ArtifactInvalid,
    OutputLimit,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalManifest {
    pub fixture: Fixture,
    pub recipe: String,
    pub catalogue: String,
    pub resource_model: String,
    pub procedure: String,
    pub bundle: String,
    pub policy: String,
    pub plan: Plan,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EngineGrant<P = Plan> {
    pub version: u8,
    pub kind: String,
    pub launch_id: String,
    pub manifest: String,
    pub bundle: String,
    pub numerical_bundle: String,
    pub recipe: String,
    pub procedure: String,
    pub fixture: Fixture,
    pub input_icc_sha256: String,
    pub output_icc_sha256: String,
    pub plan: P,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ExecutionPlan {
    Measurement(Plan),
    Qualified(crate::qualified::Plan),
}
pub type RuntimeGrant = EngineGrant<ExecutionPlan>;

impl ExecutionPlan {
    pub fn measurement(&self) -> Option<&Plan> {
        match self {
            Self::Measurement(plan) => Some(plan),
            Self::Qualified(_) => None,
        }
    }
    pub fn qualified(&self) -> Option<&crate::qualified::Plan> {
        match self {
            Self::Qualified(plan) => Some(plan),
            Self::Measurement(_) => None,
        }
    }
}
impl<P> EngineGrant<P> {
    pub fn map_plan<Q>(self, convert: impl FnOnce(P) -> Q) -> EngineGrant<Q> {
        EngineGrant {
            version: self.version,
            kind: self.kind,
            launch_id: self.launch_id,
            manifest: self.manifest,
            bundle: self.bundle,
            numerical_bundle: self.numerical_bundle,
            recipe: self.recipe,
            procedure: self.procedure,
            fixture: self.fixture,
            input_icc_sha256: self.input_icc_sha256,
            output_icc_sha256: self.output_icc_sha256,
            plan: convert(self.plan),
        }
    }
    fn validate_header(&self, version: u8, kind: &str) -> Result<()> {
        if self.version != version
            || self.kind != kind
            || !hex(&self.launch_id, 32)
            || self.procedure != PROCEDURE
            || [
                &self.manifest,
                &self.bundle,
                &self.numerical_bundle,
                &self.recipe,
                &self.input_icc_sha256,
                &self.output_icc_sha256,
            ]
            .iter()
            .any(|s| !hex(s, 64))
        {
            return Err(ErrorCode::InvalidRequest);
        }
        self.fixture.validate()?;
        Ok(())
    }
}
impl EngineGrant<Plan> {
    pub fn validate(&self) -> Result<()> {
        self.validate_header(2, "film-measurement-grant")?;
        self.plan.validate(&self.fixture)
    }
}
impl RuntimeGrant {
    pub fn validate(&self) -> Result<()> {
        match &self.plan {
            ExecutionPlan::Measurement(plan) => {
                self.validate_header(2, "film-measurement-grant")?;
                plan.validate(&self.fixture)
            }
            ExecutionPlan::Qualified(plan) => {
                self.validate_header(3, "film-qualified-grant")?;
                plan.validate(&self.fixture)
            }
        }
    }
}
impl Plan {
    fn validate(&self, fixture: &Fixture) -> Result<()> {
        let n = geometry(fixture.width, fixture.height)?;
        if self.formula != FORMULA
            || self.width != fixture.width
            || self.height != fixture.height
            || self.source_cache_bytes != fixture.source_bytes()
            || self.storage_reserve_bytes != STORAGE
            || self.gamut != local_plan("cam16ucs-srgb-f64-v1", n, fixture.width, GAMUT_ALLOWANCE)?
            || self.cctf != local_plan("srgb-cctf-f64-v1", n, fixture.width, GAMUT_ALLOWANCE)?
            || self.jpeg != local_plan("jpeg-uint8-rows-v1", n, fixture.width, JPEG_ALLOWANCE)?
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Prediction::Unqualified { missing, .. } = &self.prediction {
            let keys: BTreeSet<_> = missing
                .iter()
                .map(|entry| format!("{:?}:{:?}", entry.stage, entry.term))
                .collect();
            if missing.is_empty() || missing.len() > 60 || keys.len() != missing.len() {
                return Err(ErrorCode::InvalidRequest);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StageOffer {
    pub version: u8,
    pub kind: String,
    pub launch_id: String,
    pub source: String,
    pub source_bytes: u64,
    #[serde(deserialize_with = "required")]
    pub source_sha256: Option<String>,
    #[serde(deserialize_with = "required")]
    pub destination_device: Option<u64>,
    #[serde(deserialize_with = "required")]
    pub destination_inode: Option<u64>,
    pub result_device: u64,
    pub result_inode: u64,
    pub grant_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StageAck {
    pub version: u8,
    pub kind: String,
    pub launch_id: String,
    pub source_bytes: u64,
    #[serde(deserialize_with = "required")]
    pub source_sha256: Option<String>,
    #[serde(deserialize_with = "required")]
    pub destination_device: Option<u64>,
    #[serde(deserialize_with = "required")]
    pub destination_inode: Option<u64>,
    pub grant_sha256: String,
}
impl StageOffer {
    pub fn ack(&self) -> StageAck {
        StageAck {
            version: 2,
            kind: "staged".into(),
            launch_id: self.launch_id.clone(),
            source_bytes: self.source_bytes,
            source_sha256: self.source_sha256.clone(),
            destination_device: self.destination_device,
            destination_inode: self.destination_inode,
            grant_sha256: self.grant_sha256.clone(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permit {
    pub version: u8,
    pub kind: String,
    pub launch_id: String,
    pub grant_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PixelObservations {
    pub input_pixels_sha256: String,
    pub film_pixels_sha256: String,
    pub width: u64,
    pub height: u64,
    pub icc_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub input_pixels_sha256: String,
    pub film_pixels_sha256: String,
    pub jpeg_sha256: String,
    pub jpeg_bytes: u64,
    pub width: u64,
    pub height: u64,
    pub icc_sha256: String,
    pub reference_evidence_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub stage: Stage,
    pub elapsed_us: u64,
    pub reclaim_us: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProducerSuccess {
    pub version: u8,
    pub kind: String,
    pub outcome: String,
    pub launch_id: String,
    pub manifest: String,
    pub plan_sha256: String,
    pub pixels: PixelObservations,
    pub stages: Vec<Timing>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProducerFailure {
    pub version: u8,
    pub kind: String,
    pub outcome: Outcome,
    #[serde(deserialize_with = "required")]
    pub detail: Option<Detail>,
    pub launch_id: String,
    pub manifest: String,
    pub plan_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ProducerResult {
    Success(ProducerSuccess),
    Failure(ProducerFailure),
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerSuccess {
    pub version: u8,
    pub kind: String,
    pub outcome: Outcome,
    pub launch_id: String,
    pub manifest: String,
    pub plan_sha256: String,
    pub artifact: Artifact,
    pub execution_us: u64,
    pub stages: Vec<Timing>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerFailure {
    pub version: u8,
    pub kind: String,
    pub outcome: Outcome,
    #[serde(deserialize_with = "required")]
    pub detail: Option<Detail>,
    pub phase: Phase,
    pub launch_id: String,
    pub manifest: String,
    pub plan_sha256: String,
    pub execution_us: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WorkerResult {
    Success(WorkerSuccess),
    Failure(WorkerFailure),
}
impl WorkerResult {
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Success(v) => v.outcome,
            Self::Failure(v) => v.outcome,
        }
    }
    pub fn detail(&self) -> Option<Detail> {
        match self {
            Self::Success(_) => None,
            Self::Failure(v) => v.detail,
        }
    }
    pub fn validate<P: Serialize>(&self, grant: &EngineGrant<P>) -> Result<()> {
        let (version, kind, launch, manifest, plan) = match self {
            Self::Success(v) => (
                v.version,
                &v.kind,
                &v.launch_id,
                &v.manifest,
                &v.plan_sha256,
            ),
            Self::Failure(v) => (
                v.version,
                &v.kind,
                &v.launch_id,
                &v.manifest,
                &v.plan_sha256,
            ),
        };
        if version != 2
            || kind != "film-measurement-result"
            || launch != &grant.launch_id
            || manifest != &grant.manifest
            || *plan != hash(&grant.plan)?
        {
            return Err(ErrorCode::InvalidRequest);
        }
        match self {
            Self::Success(v) => {
                let r = &grant.fixture.reference;
                let a = &v.artifact;
                if v.outcome != Outcome::Completed
                    || a.input_pixels_sha256 != r.input_pixels_sha256
                    || a.film_pixels_sha256 != r.film_pixels_sha256
                    || a.jpeg_sha256 != r.jpeg_sha256
                    || a.jpeg_bytes != r.jpeg_bytes
                    || a.reference_evidence_sha256 != r.evidence_sha256
                    || a.width != grant.fixture.width
                    || a.height != grant.fixture.height
                    || a.icc_sha256 != grant.output_icc_sha256
                {
                    return Err(ErrorCode::InvalidRequest);
                }
                timings(&v.stages, false)?;
            }
            Self::Failure(v) => {
                if ![
                    Outcome::AllocationFailed,
                    Outcome::StorageFull,
                    Outcome::Deadline,
                    Outcome::EngineFailed,
                ]
                .contains(&v.outcome)
                {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
        }
        Ok(())
    }
}
pub fn timings(stages: &[Timing], producer: bool) -> Result<()> {
    let mut seen = BTreeSet::new();
    if stages.len() > if producer { 10 } else { 12 } {
        return Err(ErrorCode::InvalidRequest);
    }
    for t in stages {
        if !seen.insert(t.stage)
            || producer && [Stage::Staging, Stage::Validation].contains(&t.stage)
        {
            return Err(ErrorCode::InvalidRequest);
        }
    }
    Ok(())
}

pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    // serde_json's default Map is BTreeMap; recursively converting to Value sorts every object.
    let value = serde_json::to_value(value).map_err(|_| ErrorCode::InvalidRequest)?;
    serde_json::to_vec(&value).map_err(|_| ErrorCode::InvalidRequest)
}
pub fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(digest(&canonical(value)?))
}
pub fn parse<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T> {
    if bytes.len() > limit {
        return Err(ErrorCode::InvalidRequest);
    }
    serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)
}
fn image(value: &str) -> bool {
    if value.len() > 512 {
        return false;
    }
    let id = value.strip_prefix("sha256:").or_else(|| {
        let (repository, digest) = value.split_once("@sha256:")?;
        (!repository.is_empty()
            && repository.as_bytes()[0].is_ascii_alphanumeric()
            && repository
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._-:".contains(&b)))
        .then_some(digest)
    });
    id.is_some_and(|id| hex(id, 64))
}

pub(crate) fn metadata_file<T: DeserializeOwned>(
    path: &Path,
    limit: usize,
    expected: &str,
) -> Result<T> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ErrorCode::Unavailable)?;
    let meta = file.metadata().map_err(|_| ErrorCode::Unavailable)?;
    if !meta.is_file()
        || meta.uid() != 0
        || meta.nlink() != 1
        || meta.mode() & 0o022 != 0
        || meta.len() > limit as u64
    {
        return Err(ErrorCode::Unavailable);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Unavailable)?;
    if bytes.len() > limit || digest(&bytes) != expected {
        return Err(ErrorCode::InvalidRequest);
    }
    parse(&bytes, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_plans_are_checked_and_preserve_rows() {
        assert_eq!(
            local_plan("cam16ucs-srgb-f64-v1", 1, 1, GAMUT_ALLOWANCE)
                .unwrap()
                .scratch_bytes,
            67_110_912
        );
        assert_eq!(
            local_plan("srgb-cctf-f64-v1", 262145, 1, GAMUT_ALLOWANCE)
                .unwrap()
                .batch_pixels,
            262144
        );
        assert_eq!(
            local_plan("jpeg-uint8-rows-v1", 9568 * 9568, 9568, JPEG_ALLOWANCE)
                .unwrap()
                .batch_pixels,
            258336
        );
        for (n, w, a) in [
            (0, 1, JPEG_ALLOWANCE),
            (1, 0, JPEG_ALLOWANCE),
            (300000, 300000, JPEG_ALLOWANCE),
            (u64::MAX, 1, JPEG_ALLOWANCE),
        ] {
            assert!(local_plan("jpeg-uint8-rows-v1", n, w, a).is_err());
        }
        assert!(geometry(9569, 1).is_err());
        assert!(geometry(u64::MAX, 2).is_err());
    }
    #[test]
    fn strict_requests_do_not_accept_numeric_or_object_shortcuts() {
        let good = format!(
            r#"{{"version":2,"op":"inspect","instance":"{}","incarnation":"{}","sequence":1}}"#,
            "0".repeat(32),
            "1".repeat(32)
        );
        assert!(Request::parse(good.as_bytes()).is_ok());
        for replacement in ["true", "-1", "1.0", "1e0", "18446744073709551616"] {
            assert!(
                Request::parse(
                    good.replace("\"sequence\":1", &format!("\"sequence\":{replacement}"))
                        .as_bytes()
                )
                .is_err()
            );
        }
        assert!(
            Request::parse(
                good.replace("\"version\":2", "\"version\":2,\"version\":2")
                    .as_bytes()
            )
            .is_err()
        );
        assert!(Request::parse(format!("{good}{{}}").as_bytes()).is_err());
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub incarnation: String,
    pub sequence: u64,
    pub workload: Workload,
    pub policy: String,
    pub bundle: String,
    pub catalogue: String,
    pub resource_model: String,
    pub manifest: String,
    pub state: crate::protocol::State,
    pub phase: Phase,
    pub cancellation_requested: bool,
    pub accepted_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub outcome: Option<Outcome>,
    #[serde(deserialize_with = "required")]
    pub detail: Option<Detail>,
    pub runtime: Option<crate::protocol::Runtime>,
    pub limits: Limits,
    pub evidence: Option<crate::protocol::Evidence>,
    pub plan: Plan,
    pub result: Option<WorkerResult>,
    pub cleanup: crate::protocol::Cleanup,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResultBody {
    Capability {
        capability: String,
        instance: String,
        incarnation: String,
        next_sequence: u64,
        policy: String,
        bundle: String,
        catalogue: String,
        resource_model: String,
        availability: crate::protocol::Availability,
        active: Option<Receipt>,
    },
    Receipt {
        receipt: Receipt,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Captured {
    pub catalogue: String,
    // The historical internal key remains readable for v2 registries. The
    // validated grant variant selects component-model or qualified-envelope
    // identity; v3 public receipts always name it `envelope`.
    pub resource_model: String,
    pub grant: RuntimeGrant,
    pub phase: Phase,
    pub stage_release_intent: bool,
    pub engine_release_intent: bool,
    pub grant_file: Option<FileIdentity>,
    pub snapshot: Option<FileIdentity>,
    pub result_file: Option<FileIdentity>,
    pub result: Option<WorkerResult>,
    #[serde(deserialize_with = "required")]
    pub detail: Option<Detail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualification_failure: Option<crate::qualified::QualificationFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualification_observation_valid: Option<bool>,
}
impl Captured {
    pub fn qualified_receipt(
        &self,
        common: &crate::protocol::Receipt,
    ) -> crate::qualified::Receipt {
        crate::qualified::Receipt {
            incarnation: common.incarnation.clone(),
            sequence: common.sequence,
            workload: Workload {
                kind: "film-fixture".into(),
                fixture_id: self.grant.fixture.id.clone(),
            },
            policy: common.policy.clone(),
            bundle: common.bundle.clone(),
            catalogue: self.catalogue.clone(),
            envelope: self.resource_model.clone(),
            manifest: self.grant.manifest.clone(),
            state: common.state,
            phase: self.phase,
            cancellation_requested: common.cancellation_requested,
            accepted_at_unix_ms: common.accepted_at_unix_ms,
            deadline_unix_ms: common.deadline_unix_ms,
            outcome: common.outcome,
            detail: self.detail,
            runtime: common.runtime.clone(),
            limits: common.limits.clone(),
            evidence: common.evidence.clone(),
            plan: self
                .grant
                .plan
                .qualified()
                .expect("validated qualified capture")
                .clone(),
            result: self.result.clone(),
            cleanup: common.cleanup,
            qualification_failure: self.qualification_failure,
        }
    }
    pub fn receipt(&self, common: &crate::protocol::Receipt) -> Receipt {
        Receipt {
            incarnation: common.incarnation.clone(),
            sequence: common.sequence,
            workload: Workload {
                kind: "film-fixture".into(),
                fixture_id: self.grant.fixture.id.clone(),
            },
            policy: common.policy.clone(),
            bundle: common.bundle.clone(),
            catalogue: self.catalogue.clone(),
            resource_model: self.resource_model.clone(),
            manifest: self.grant.manifest.clone(),
            state: common.state,
            phase: self.phase,
            cancellation_requested: common.cancellation_requested,
            accepted_at_unix_ms: common.accepted_at_unix_ms,
            deadline_unix_ms: common.deadline_unix_ms,
            outcome: common.outcome,
            detail: self.detail,
            runtime: common.runtime.clone(),
            limits: common.limits.clone(),
            evidence: common.evidence.clone(),
            plan: self
                .grant
                .plan
                .measurement()
                .expect("validated measurement capture")
                .clone(),
            result: self.result.clone(),
            cleanup: common.cleanup,
        }
    }
}

pub(crate) fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

#[cfg(test)]
pub(crate) fn test_grant() -> EngineGrant {
    let fixture = Fixture {
        id: "a".repeat(32),
        width: 19,
        height: 17,
        source: Source::SyntheticRgb {
            generator: "linear-rgb-f32-v1".into(),
            pattern: Pattern::Noise,
            seed: u32::MAX as u64,
        },
        reference: Reference {
            input_pixels_sha256: "b".repeat(64),
            film_pixels_sha256: "c".repeat(64),
            jpeg_sha256: "d".repeat(64),
            jpeg_bytes: 512 * 1024 * 1024,
            evidence_sha256: "e".repeat(64),
        },
    };
    let case = ModelCase {
        fixture_id: fixture.id.clone(),
        stages: STAGES
            .iter()
            .map(|stage| StageBounds {
                stage: *stage,
                runtime: Bound::Unknown,
                native: Bound::Unknown,
                allocator_retention: Bound::Unknown,
                kernel: Bound::Unknown,
            })
            .collect(),
    };
    EngineGrant {
        version: 2,
        kind: "film-measurement-grant".into(),
        launch_id: "f".repeat(32),
        manifest: "1".repeat(64),
        bundle: "2".repeat(64),
        numerical_bundle: "3".repeat(64),
        recipe: "4".repeat(64),
        procedure: PROCEDURE.into(),
        fixture: fixture.clone(),
        input_icc_sha256: "5".repeat(64),
        output_icc_sha256: "6".repeat(64),
        plan: plan(&fixture, &case, 8 * 1024 * 1024 * 1024).unwrap(),
    }
}
#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn shared_runtime_grant_preserves_each_authority_and_full_plan_hash() {
        let measurement = test_grant();
        let original = canonical(&measurement).unwrap();
        let runtime = measurement.clone().map_plan(ExecutionPlan::Measurement);
        assert_eq!(original, canonical(&runtime).unwrap());
        runtime.validate().unwrap();
        let fixture = measurement.fixture.clone();
        let qualified = crate::qualified::Plan::calculate(
            &fixture,
            &crate::qualified::Case::Qualified {
                fixture_id: fixture.id.clone(),
                empirical_ceiling_bytes: 1 << 30,
                safety_reserve_bytes: 1 << 20,
                evidence_sha256: "a".repeat(64),
            },
            &"b".repeat(64),
            &"c".repeat(64),
            8 << 30,
        )
        .unwrap();
        let mut grant = measurement
            .clone()
            .map_plan(|_| ExecutionPlan::Qualified(qualified));
        grant.version = 3;
        grant.kind = "film-qualified-grant".into();
        grant.validate().unwrap();
        let bytes = canonical(&grant).unwrap();
        assert!(parse::<EngineGrant>(&bytes, FRAME).is_err());
        let parsed: RuntimeGrant = parse(&bytes, FRAME).unwrap();
        assert_eq!(parsed, grant);
        assert_eq!(hash(&parsed.plan).unwrap(), hash(&grant.plan).unwrap());
        assert_ne!(
            hash(&parsed.plan).unwrap(),
            hash(&measurement.plan).unwrap()
        );
        let mut changed = grant.clone();
        changed.version = 2;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        let mut changed = grant.clone();
        changed.kind = "film-measurement-grant".into();
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        let mut changed = grant.clone();
        if let ExecutionPlan::Qualified(plan) = &mut changed.plan {
            plan.known_required_bytes -= 1;
        }
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        let mut result = WorkerResult::Failure(WorkerFailure {
            version: 2,
            kind: "film-measurement-result".into(),
            outcome: Outcome::EngineFailed,
            detail: None,
            phase: Phase::Engine,
            launch_id: grant.launch_id.clone(),
            manifest: grant.manifest.clone(),
            plan_sha256: hash(&grant.plan).unwrap(),
            execution_us: 1,
        });
        result.validate(&grant).unwrap();
        if let WorkerResult::Failure(failure) = &mut result {
            failure.plan_sha256 = hash(&measurement.plan).unwrap();
        }
        assert_eq!(result.validate(&grant), Err(ErrorCode::InvalidRequest));
    }
    #[test]
    fn synthetic_seed_matches_the_bounded_catalogue_contract() {
        let mut fixture = test_grant().fixture;
        fixture.validate().unwrap();
        if let Source::SyntheticRgb { seed, .. } = &mut fixture.source {
            *seed = u32::MAX as u64 + 1;
        }
        assert_eq!(fixture.validate(), Err(ErrorCode::InvalidRequest));
        if let Source::SyntheticRgb { seed, pattern, .. } = &mut fixture.source {
            *seed = 1;
            *pattern = Pattern::Gradient;
        }
        assert_eq!(fixture.validate(), Err(ErrorCode::InvalidRequest));
        if let Source::SyntheticRgb { seed, .. } = &mut fixture.source {
            *seed = 0;
        }
        fixture.validate().unwrap();
    }
    #[test]
    fn canonical_encoding_matches_shared_reviewed_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../design/schemas/processing-film-measurement-canonical-vectors.json"
        ))
        .unwrap();
        for vector in vectors["cases"].as_array().unwrap() {
            let value =
                serde_json::from_str::<serde_json::Value>(vector["input_json"].as_str().unwrap());
            if vector.get("error").is_some() {
                assert!(value.is_err());
                continue;
            }
            let bytes = canonical(&value.unwrap()).unwrap();
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, vector["canonical_utf8_hex"].as_str().unwrap());
            assert_eq!(digest(&bytes), vector["sha256"].as_str().unwrap());
        }
    }
    #[test]
    fn all_shared_planner_vectors_match_the_pinned_engine() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tools/processing/film/local-plan-vectors.json"
        ))
        .unwrap();
        assert_eq!(vectors["cases"].as_array().unwrap().len(), 37);
        for vector in vectors["cases"].as_array().unwrap() {
            let model = match vector["operation"].as_str().unwrap() {
                "gamut" => "cam16ucs-srgb-f64-v1",
                "cctf" => "srgb-cctf-f64-v1",
                _ => "jpeg-uint8-rows-v1",
            };
            let result = vector["pixel_count"]
                .as_u64()
                .zip(vector["workspace_bytes"].as_u64())
                .ok_or(ErrorCode::UnsupportedFixture)
                .and_then(|(n, a)| {
                    local_plan(
                        model,
                        n,
                        vector.get("width").and_then(|w| w.as_u64()).unwrap_or(1),
                        a,
                    )
                });
            if vector.get("error").is_some() {
                assert!(result.is_err(), "{}", vector["id"]);
                continue;
            }
            let result = result.unwrap();
            let expected = &vector["expected"];
            assert_eq!(result.model, expected["model"].as_str().unwrap());
            assert_eq!(
                result.allowance_bytes,
                expected["workspace_allowance_bytes"].as_u64().unwrap()
            );
            assert_eq!(
                result.scratch_bytes,
                expected["scratch_bytes"].as_u64().unwrap()
            );
            assert_eq!(
                result.batch_pixels,
                expected["batch_pixels"].as_u64().unwrap()
            );
            assert_eq!(
                result.destination_bytes,
                expected["destination_bytes"].as_u64().unwrap()
            );
        }
    }
    #[test]
    fn resource_model_accepts_reordered_stages_but_rejects_duplicates_or_omissions() {
        let grant = test_grant();
        let fixture = &grant.fixture;
        let mut case = ModelCase {
            fixture_id: fixture.id.clone(),
            stages: STAGES
                .iter()
                .enumerate()
                .map(|(index, stage)| StageBounds {
                    stage: *stage,
                    runtime: Bound::Qualified {
                        bytes: index as u64,
                        evidence_sha256: "f".repeat(64),
                    },
                    native: Bound::Unknown,
                    allocator_retention: Bound::Unknown,
                    kernel: Bound::Unknown,
                })
                .collect(),
        };
        let expected = plan(fixture, &case, 8 * 1024 * 1024 * 1024).unwrap();
        case.stages.reverse();
        assert_eq!(
            plan(fixture, &case, 8 * 1024 * 1024 * 1024).unwrap(),
            expected
        );
        let saved = case.stages[0].clone();
        case.stages[0] = case.stages[1].clone();
        assert!(plan(fixture, &case, u64::MAX).is_err());
        case.stages[0] = saved;
        case.stages.pop();
        assert!(plan(fixture, &case, u64::MAX).is_err());
    }
    #[test]
    fn unknown_inventory_never_becomes_admission_and_sums_cannot_wrap() {
        let grant = test_grant();
        let fixture = grant.fixture;
        let qualified = Bound::Qualified {
            bytes: 0,
            evidence_sha256: "f".repeat(64),
        };
        let mut case = ModelCase {
            fixture_id: fixture.id.clone(),
            stages: STAGES
                .iter()
                .map(|stage| StageBounds {
                    stage: *stage,
                    runtime: qualified.clone(),
                    native: qualified.clone(),
                    allocator_retention: qualified.clone(),
                    kernel: qualified.clone(),
                })
                .collect(),
        };
        let result = plan(&fixture, &case, 1).unwrap();
        let Prediction::Unqualified {
            known_terms_exceed_limit,
            missing,
            ..
        } = result.prediction
        else {
            panic!("unproven arrays must stay unknown")
        };
        assert!(known_terms_exceed_limit);
        assert!(missing.iter().all(|m| m.term == Term::OwnedArrays));
        case.stages[0].native = Bound::Qualified {
            bytes: u64::MAX,
            evidence_sha256: "e".repeat(64),
        };
        assert!(plan(&fixture, &case, u64::MAX).is_err());
        case.stages.swap(0, 1);
        assert!(plan(&fixture, &case, u64::MAX).is_err());
    }
    #[test]
    fn null_is_required_and_producer_timings_cannot_claim_native_stages() {
        let grant = test_grant();
        let failure = WorkerFailure {
            version: 2,
            kind: "film-measurement-result".into(),
            outcome: Outcome::EngineFailed,
            detail: None,
            phase: Phase::Staging,
            launch_id: grant.launch_id,
            manifest: grant.manifest,
            plan_sha256: hash(&grant.plan).unwrap(),
            execution_us: 0,
        };
        let mut value = serde_json::to_value(failure).unwrap();
        value.as_object_mut().unwrap().remove("detail");
        assert!(serde_json::from_value::<WorkerFailure>(value).is_err());
        assert!(
            timings(
                &[Timing {
                    stage: Stage::Staging,
                    elapsed_us: 0,
                    reclaim_us: 0
                }],
                true
            )
            .is_err()
        );
        assert!(
            timings(
                &[
                    Timing {
                        stage: Stage::Decode,
                        elapsed_us: 1,
                        reclaim_us: 1
                    },
                    Timing {
                        stage: Stage::Decode,
                        elapsed_us: 2,
                        reclaim_us: 2
                    }
                ],
                true
            )
            .is_err()
        );
    }
    #[test]
    fn documented_film_examples_parse_as_written() {
        let text = include_str!("../../../design/processing-film-measurement.md");
        for (heading, body) in text.split("### ").skip(1).filter_map(|part| {
            let (h, rest) = part.split_once('\n')?;
            let (_, rest) = rest.split_once("```json\n")?;
            let (json, _) = rest.split_once("\n```")?;
            Some((h, json))
        }) {
            match heading {
                "Config" => {
                    Config::parse(body.as_bytes()).unwrap();
                }
                "Catalogue" => {
                    parse::<Catalogue>(body.as_bytes(), 32768)
                        .unwrap()
                        .validate()
                        .unwrap();
                }
                "StageBounds" => {
                    parse::<StageBounds>(body.as_bytes(), FRAME).unwrap();
                }
                "Start" => {
                    Request::parse(body.as_bytes()).unwrap();
                }
                "WorkerFailure" => {
                    parse::<WorkerFailure>(body.as_bytes(), FRAME).unwrap();
                }
                "WorkerSuccess" => {
                    parse::<WorkerSuccess>(body.as_bytes(), FRAME).unwrap();
                }
                "Response" => {
                    let v: serde_json::Value = serde_json::from_str(body).unwrap();
                    serde_json::from_value::<ResultBody>(v["result"].clone()).unwrap();
                }
                other => panic!("unconsumed example {other}"),
            }
        }
    }
}
