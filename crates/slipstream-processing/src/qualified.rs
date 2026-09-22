//! Qualified admission over exact registered Film fixtures. No image data or
//! mutable execution authority enters the planner.
use crate::{
    film::{self, Catalogue, Fixture, LocalPlan, MissingTerm, Result, Stage, Term},
    protocol::{ErrorCode, hex},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

pub const PROFILE: &str = "slipstream-film-qualified-fixtures-v1";
pub const FORMULA: &str = "film-total-envelope-v1";
pub const INVENTORY: &str = "film-known-storage-local-v1";
pub const INVALIDATIONS: usize = 256;

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
    pub envelope_sha256: String,
}
impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Self = film::parse(bytes, film::FRAME)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 3 || self.mode != "film-qualified-fixtures" {
            return Err(ErrorCode::InvalidRequest);
        }
        // The two Film authorities share the same fixed packaging, path and
        // resource constraints. This value is only used to validate those
        // settings; it never creates measurement authority or an executor.
        self.shared_settings().validate()
    }
    fn shared_settings(&self) -> film::Config {
        film::Config {
            version: 2,
            mode: "film-measurement".into(),
            instance: self.instance.clone(),
            root: self.root.clone(),
            socket: self.socket.clone(),
            peer_uid: self.peer_uid,
            image: self.image.clone(),
            memory_bytes: self.memory_bytes,
            receipt_retention_seconds: self.receipt_retention_seconds,
            catalogue_sha256: self.catalogue_sha256.clone(),
            resource_model_sha256: self.envelope_sha256.clone(),
        }
    }
    pub fn authority(&self) -> crate::protocol::Config {
        let mut config = self.shared_settings().authority();
        config.version = 3;
        config.mode = self.mode.clone();
        config
    }
    pub fn policy(&self, image: &str) -> Result<String> {
        film::hash(&serde_json::json!({"config": self, "resolved_image": image}))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub machine: String,
    pub kernel_release: String,
    pub page_bytes: u64,
    pub cpu_sha256: String,
    pub manager_sha256: String,
}
impl Environment {
    pub fn validate(&self) -> Result<()> {
        if self.machine != "x86_64"
            || self.kernel_release.is_empty()
            || self.kernel_release.len() > 256
            || !self
                .kernel_release
                .bytes()
                .all(|c| (b'!'..=b'~').contains(&c))
            || !(1..=65536).contains(&self.page_bytes)
            || !hex(&self.cpu_sha256, 64)
            || !hex(&self.manager_sha256, 64)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Case {
    Unknown {
        fixture_id: String,
    },
    Qualified {
        fixture_id: String,
        empirical_ceiling_bytes: u64,
        safety_reserve_bytes: u64,
        evidence_sha256: String,
    },
}
impl Case {
    pub fn fixture_id(&self) -> &str {
        match self {
            Self::Unknown { fixture_id } | Self::Qualified { fixture_id, .. } => fixture_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u8,
    pub formula: String,
    pub inventory: String,
    pub catalogue_sha256: String,
    pub image: String,
    pub launcher_sha256: String,
    pub environment: Environment,
    pub cases: Vec<Case>,
}
impl Envelope {
    pub fn validate(&self, catalogue: &Catalogue, catalogue_sha256: &str) -> Result<()> {
        catalogue.validate()?;
        self.environment.validate()?;
        if self.version != 1
            || self.formula != FORMULA
            || self.inventory != INVENTORY
            || !hex(&self.catalogue_sha256, 64)
            || self.catalogue_sha256 != catalogue_sha256
            || !self
                .image
                .strip_prefix("sha256:")
                .is_some_and(|v| hex(v, 64))
            || !hex(&self.launcher_sha256, 64)
            || self.cases.len() > 16
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            let fixture = catalogue
                .fixtures
                .iter()
                .find(|f| f.id == case.fixture_id())
                .ok_or(ErrorCode::InvalidRequest)?;
            if !ids.insert(case.fixture_id()) {
                return Err(ErrorCode::InvalidRequest);
            }
            if let Case::Qualified {
                empirical_ceiling_bytes,
                safety_reserve_bytes,
                evidence_sha256,
                ..
            } = case
            {
                if !hex(evidence_sha256, 64) {
                    return Err(ErrorCode::InvalidRequest);
                }
                // An over-budget case is valid data and will be refused by
                // admission. An overflowing or nonpositive bound is invalid.
                required_bytes(
                    0,
                    *empirical_ceiling_bytes,
                    fixture.source_bytes(),
                    *safety_reserve_bytes,
                )?;
            }
        }
        Ok(())
    }
    pub fn matches(&self, image: &str, launcher: &str, environment: &Environment) -> Result<()> {
        if self.image != image
            || self.launcher_sha256 != launcher
            || self.environment != *environment
        {
            return Err(ErrorCode::Unavailable);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Documents {
    pub catalogue: Catalogue,
    pub envelope: Envelope,
}
impl Documents {
    pub fn load(config: &Config) -> Result<Self> {
        let root = Path::new(&config.root);
        let catalogue: Catalogue = film::metadata_file(
            &root.join("catalogue.json"),
            32768,
            &config.catalogue_sha256,
        )?;
        let envelope: Envelope =
            film::metadata_file(&root.join("envelope.json"), 131072, &config.envelope_sha256)?;
        envelope.validate(&catalogue, &config.catalogue_sha256)?;
        Ok(Self {
            catalogue,
            envelope,
        })
    }
    pub fn plan(&self, id: &str, envelope_sha256: &str, memory: u64) -> Result<(Fixture, Plan)> {
        let fixture = self
            .catalogue
            .fixtures
            .iter()
            .find(|f| f.id == id)
            .ok_or(ErrorCode::UnknownFixture)?;
        film::geometry(fixture.width, fixture.height)?;
        let case = self
            .envelope
            .cases
            .iter()
            .find(|c| c.fixture_id() == id)
            .ok_or(ErrorCode::OutsideEnvelope)?;
        let plan = Plan::calculate(
            fixture,
            case,
            envelope_sha256,
            &film::hash(&self.envelope.environment)?,
            memory,
        )?;
        Ok((fixture.clone(), plan))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub formula: String,
    pub inventory: String,
    pub width: u64,
    pub height: u64,
    pub envelope_sha256: String,
    pub evidence_sha256: String,
    pub environment_sha256: String,
    pub known_required_bytes: u64,
    pub source_cache_bytes: u64,
    pub storage_reserve_bytes: u64,
    pub empirical_ceiling_bytes: u64,
    pub safety_reserve_bytes: u64,
    pub required_bytes: u64,
    pub attempt_limit_bytes: u64,
    pub missing: Vec<MissingTerm>,
    pub gamut: LocalPlan,
    pub cctf: LocalPlan,
    pub jpeg: LocalPlan,
}
impl Plan {
    pub fn calculate(
        fixture: &Fixture,
        case: &Case,
        envelope: &str,
        environment: &str,
        memory: u64,
    ) -> Result<Self> {
        fixture.validate()?;
        if case.fixture_id() != fixture.id || !hex(envelope, 64) || !hex(environment, 64) {
            return Err(ErrorCode::InvalidRequest);
        }
        let Case::Qualified {
            empirical_ceiling_bytes,
            safety_reserve_bytes,
            evidence_sha256,
            ..
        } = case
        else {
            return Err(ErrorCode::UnqualifiedEnvelope);
        };
        if !hex(evidence_sha256, 64) {
            return Err(ErrorCode::InvalidRequest);
        }
        let n = film::geometry(fixture.width, fixture.height)?;
        let gamut = film::local_plan(
            "cam16ucs-srgb-f64-v1",
            n,
            fixture.width,
            film::GAMUT_ALLOWANCE,
        )?;
        let cctf = film::local_plan("srgb-cctf-f64-v1", n, fixture.width, film::GAMUT_ALLOWANCE)?;
        let jpeg = film::local_plan("jpeg-uint8-rows-v1", n, fixture.width, film::JPEG_ALLOWANCE)?;
        let known_required_bytes = film::STORAGE
            .checked_add(fixture.source_bytes())
            .and_then(|v| {
                v.checked_add(
                    65536
                        .max(gamut.scratch_bytes)
                        .max(cctf.scratch_bytes)
                        .max(jpeg.scratch_bytes),
                )
            })
            .ok_or(ErrorCode::InvalidRequest)?;
        let missing = film::STAGES
            .into_iter()
            .flat_map(|stage| {
                [
                    Term::OwnedArrays,
                    Term::Runtime,
                    Term::Native,
                    Term::AllocatorRetention,
                    Term::Kernel,
                ]
                .into_iter()
                .filter(move |term| {
                    *term != Term::OwnedArrays
                        || ![Stage::Staging, Stage::Validation].contains(&stage)
                })
                .map(move |term| MissingTerm { stage, term })
            })
            .collect();
        let required_bytes = required_bytes(
            known_required_bytes,
            *empirical_ceiling_bytes,
            fixture.source_bytes(),
            *safety_reserve_bytes,
        )?;
        if required_bytes > memory {
            return Err(ErrorCode::ResourceBudget);
        }
        Ok(Self {
            formula: FORMULA.into(),
            inventory: INVENTORY.into(),
            width: fixture.width,
            height: fixture.height,
            envelope_sha256: envelope.into(),
            evidence_sha256: evidence_sha256.clone(),
            environment_sha256: environment.into(),
            known_required_bytes,
            source_cache_bytes: fixture.source_bytes(),
            storage_reserve_bytes: film::STORAGE,
            empirical_ceiling_bytes: *empirical_ceiling_bytes,
            safety_reserve_bytes: *safety_reserve_bytes,
            required_bytes,
            attempt_limit_bytes: memory,
            missing,
            gamut,
            cctf,
            jpeg,
        })
    }
    pub fn validate(&self, fixture: &Fixture) -> Result<()> {
        if ![8, 12, 16, 24, 32]
            .map(|n| n * 1024 * 1024 * 1024)
            .contains(&self.attempt_limit_bytes)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let expected = Self::calculate(
            fixture,
            &Case::Qualified {
                fixture_id: fixture.id.clone(),
                empirical_ceiling_bytes: self.empirical_ceiling_bytes,
                safety_reserve_bytes: self.safety_reserve_bytes,
                evidence_sha256: self.evidence_sha256.clone(),
            },
            &self.envelope_sha256,
            &self.environment_sha256,
            self.attempt_limit_bytes,
        )?;
        if *self != expected {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

fn required_bytes(known: u64, empirical: u64, source: u64, reserve: u64) -> Result<u64> {
    if empirical == 0 || reserve == 0 {
        return Err(ErrorCode::InvalidRequest);
    }
    empirical
        .checked_add(film::STORAGE)
        .and_then(|v| v.checked_add(source))
        .map(|v| v.max(known))
        .and_then(|v| v.checked_add(reserve))
        .ok_or(ErrorCode::InvalidRequest)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QualificationFailure {
    PeakExceeded,
    ProcessingOom,
    AllocationFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Invalidation {
    pub envelope: String,
    pub fixture_id: String,
    pub reason: QualificationFailure,
    pub incarnation: String,
    pub sequence: u64,
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
        envelope: String,
        workload: film::Workload,
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
        let request: Self = film::parse(bytes, film::FRAME)?;
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
        if version != 3 || !hex(instance, 32) {
            return Err(ErrorCode::InvalidRequest);
        }
        match &request {
            Self::Start {
                incarnation,
                sequence,
                policy,
                bundle,
                catalogue,
                envelope,
                workload,
                ..
            } => {
                if !hex(incarnation, 32)
                    || *sequence == 0
                    || [policy, bundle, catalogue, envelope]
                        .iter()
                        .any(|v| !hex(v, 64))
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
    pub(crate) fn into_shared(self) -> (crate::protocol::Request, Option<(String, String)>) {
        use crate::protocol::{Request as Shared, Workload};
        match self {
            Self::Reconcile { version, instance } => {
                (Shared::Reconcile { version, instance }, None)
            }
            Self::Inspect {
                version,
                instance,
                incarnation,
                sequence,
            } => (
                Shared::Inspect {
                    version,
                    instance,
                    incarnation,
                    sequence,
                },
                None,
            ),
            Self::Cancel {
                version,
                instance,
                incarnation,
                sequence,
            } => (
                Shared::Cancel {
                    version,
                    instance,
                    incarnation,
                    sequence,
                },
                None,
            ),
            Self::Start {
                version,
                instance,
                incarnation,
                sequence,
                policy,
                bundle,
                catalogue,
                envelope,
                workload,
            } => (
                Shared::Start {
                    version,
                    instance,
                    incarnation,
                    sequence,
                    policy,
                    bundle,
                    workload: Workload::Film(workload),
                },
                Some((catalogue, envelope)),
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalManifest {
    pub fixture: Fixture,
    pub recipe: String,
    pub catalogue: String,
    pub envelope: String,
    pub procedure: String,
    pub bundle: String,
    pub policy: String,
    pub plan: Plan,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Blocked,
    Unqualified,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub incarnation: String,
    pub sequence: u64,
    pub workload: film::Workload,
    pub policy: String,
    pub bundle: String,
    pub catalogue: String,
    pub envelope: String,
    pub manifest: String,
    pub state: crate::protocol::State,
    pub phase: film::Phase,
    pub cancellation_requested: bool,
    pub accepted_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    #[serde(deserialize_with = "film::required")]
    pub outcome: Option<crate::protocol::Outcome>,
    #[serde(deserialize_with = "film::required")]
    pub detail: Option<film::Detail>,
    #[serde(deserialize_with = "film::required")]
    pub runtime: Option<crate::protocol::Runtime>,
    pub limits: crate::protocol::Limits,
    #[serde(deserialize_with = "film::required")]
    pub evidence: Option<crate::protocol::Evidence>,
    pub plan: Plan,
    #[serde(deserialize_with = "film::required")]
    pub result: Option<film::WorkerResult>,
    pub cleanup: crate::protocol::Cleanup,
    #[serde(deserialize_with = "film::required")]
    pub qualification_failure: Option<QualificationFailure>,
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
        envelope: String,
        availability: Availability,
        #[serde(deserialize_with = "film::required")]
        active: Option<Receipt>,
    },
    Receipt {
        receipt: Receipt,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../design/schemas/processing-film-envelope-vectors.json"
        ))
        .unwrap()
    }
    fn case(fixture: &Fixture, empirical: u64, reserve: u64) -> Case {
        Case::Qualified {
            fixture_id: fixture.id.clone(),
            empirical_ceiling_bytes: empirical,
            safety_reserve_bytes: reserve,
            evidence_sha256: "1".repeat(64),
        }
    }
    fn environment() -> Environment {
        Environment {
            machine: "x86_64".into(),
            kernel_release: "test-kernel".into(),
            page_bytes: 4096,
            cpu_sha256: "2".repeat(64),
            manager_sha256: "3".repeat(64),
        }
    }
    fn documents() -> Documents {
        let grant = film::test_grant();
        let catalogue = Catalogue {
            version: 1,
            numerical_bundle: grant.numerical_bundle,
            recipe: grant.recipe,
            procedure: grant.procedure,
            reference_image: format!("sha256:{}", "4".repeat(64)),
            input_icc_sha256: grant.input_icc_sha256,
            output_icc_sha256: grant.output_icc_sha256,
            fixtures: vec![grant.fixture.clone()],
        };
        let envelope = Envelope {
            version: 1,
            formula: FORMULA.into(),
            inventory: INVENTORY.into(),
            catalogue_sha256: film::hash(&catalogue).unwrap(),
            image: format!("sha256:{}", "5".repeat(64)),
            launcher_sha256: "6".repeat(64),
            environment: environment(),
            cases: vec![case(&grant.fixture, 1024 * 1024 * 1024, 1024 * 1024)],
        };
        Documents {
            catalogue,
            envelope,
        }
    }
    #[test]
    fn canonical_examples_parse_without_reinterpreting_measurement_authority() {
        let data = vectors();
        assert_eq!(data["cases"].as_array().unwrap().len(), 6);
        for vector in data["cases"].as_array().unwrap() {
            let bytes = vector["input_json"].as_str().unwrap().as_bytes();
            let value: Value = film::parse(bytes, film::FRAME).unwrap();
            let canonical = film::canonical(&value).unwrap();
            let actual: String = canonical.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(actual, vector["canonical_utf8_hex"]);
            assert_eq!(crate::protocol::digest(&canonical), vector["sha256"]);
            match vector["definition"].as_str().unwrap() {
                "Config" => {
                    let config = Config::parse(bytes).unwrap();
                    assert_eq!(config.authority().version, 3);
                    assert!(film::Config::parse(bytes).is_err());
                    for version in [1, 2] {
                        let mut changed = value.clone();
                        changed["version"] = json!(version);
                        assert!(Config::parse(&film::canonical(&changed).unwrap()).is_err());
                    }
                    let mut changed = value.clone();
                    changed["allow_unqualified"] = json!(true);
                    assert!(Config::parse(&film::canonical(&changed).unwrap()).is_err());
                }
                "Envelope" => {
                    let _: Envelope = film::parse(bytes, 131072).unwrap();
                }
                "Environment" => {
                    film::parse::<Environment>(bytes, film::FRAME)
                        .unwrap()
                        .validate()
                        .unwrap();
                }
                "QualifiedCase" => {
                    let _: Case = film::parse(bytes, film::FRAME).unwrap();
                }
                "Start" => {
                    let _: Request = Request::parse(bytes).unwrap();
                    assert!(film::Request::parse(bytes).is_err());
                }
                _ => {}
            }
        }
    }
    #[test]
    fn qualified_requests_reject_mixed_authority_and_coerced_identifiers() {
        let request = serde_json::json!({
            "op":"start", "version":3,"instance":"1".repeat(32),
            "incarnation":"2".repeat(32),"sequence":u64::MAX,
            "policy":"3".repeat(64),"bundle":"4".repeat(64),
            "catalogue":"5".repeat(64),"envelope":"6".repeat(64),
            "workload":{"kind":"film-fixture","fixture_id":"7".repeat(32)}
        });
        let bytes = film::canonical(&request).unwrap();
        Request::parse(&bytes).unwrap();
        assert!(film::Request::parse(&bytes).is_err());
        assert!(crate::protocol::Request::parse(&bytes).is_err());
        for (key, value) in [
            ("version", json!(2)),
            ("sequence", json!(0)),
            ("sequence", json!(-1)),
            ("sequence", json!(1.0)),
            ("sequence", json!(true)),
            ("instance", json!("1".repeat(31))),
            ("envelope", json!("6".repeat(65))),
            ("resource_model", json!("8".repeat(64))),
            ("allow_unqualified", json!(true)),
        ] {
            let mut changed = request.clone();
            changed[key] = value;
            assert_eq!(
                Request::parse(&film::canonical(&changed).unwrap()).unwrap_err(),
                ErrorCode::InvalidRequest
            );
        }
        let duplicate =
            String::from_utf8(bytes.clone())
                .unwrap()
                .replacen('{', "{\"version\":3,", 1);
        assert!(Request::parse(duplicate.as_bytes()).is_err());
        let mut trailing = bytes;
        trailing.extend_from_slice(b" {}");
        assert!(Request::parse(&trailing).is_err());
    }
    #[test]
    fn reviewed_arithmetic_vectors_include_each_overflow_and_exact_fit_boundary() {
        let data = vectors();
        assert_eq!(data["arithmetic"].as_array().unwrap().len(), 7);
        for v in data["arithmetic"].as_array().unwrap() {
            assert_eq!(v["storage_reserve_bytes"], film::STORAGE);
            let result = required_bytes(
                v["known_required_bytes"].as_u64().unwrap(),
                v["empirical_ceiling_bytes"].as_u64().unwrap(),
                v["source_cache_bytes"].as_u64().unwrap(),
                v["safety_reserve_bytes"].as_u64().unwrap(),
            );
            if v.get("error").is_some() {
                assert_eq!(result, Err(ErrorCode::InvalidRequest));
            } else {
                let result = result.unwrap();
                assert_eq!(result, v["required_bytes"]);
                assert_eq!(
                    result <= v["attempt_limit_bytes"].as_u64().unwrap(),
                    v["fits"].as_bool().unwrap()
                );
            }
        }
        for (e, r) in [(0, 1), (1, 0)] {
            assert!(required_bytes(1, e, 0, r).is_err());
        }
    }
    #[test]
    fn qualified_total_retains_every_unknown_component_and_source_reserve() {
        let docs = documents();
        let id = &docs.catalogue.fixtures[0].id;
        let (fixture, plan) = docs.plan(id, &"7".repeat(64), 8 << 30).unwrap();
        plan.validate(&fixture).unwrap();
        assert_eq!(plan.missing.len(), 58);
        assert_eq!(
            plan.missing
                .iter()
                .filter(|v| v.term == Term::OwnedArrays)
                .count(),
            10
        );
        for term in [
            Term::Runtime,
            Term::Native,
            Term::AllocatorRetention,
            Term::Kernel,
        ] {
            assert_eq!(plan.missing.iter().filter(|v| v.term == term).count(), 12);
        }
        assert_eq!(plan.source_cache_bytes, 0);
        let mut tiff = fixture.clone();
        tiff.source = film::Source::DevelopmentTiff {
            bytes: 734_000_000,
            sha256: "8".repeat(64),
        };
        let tiff_plan = Plan::calculate(
            &tiff,
            &case(&tiff, 1 << 30, 1 << 20),
            &plan.envelope_sha256,
            &plan.environment_sha256,
            8 << 30,
        )
        .unwrap();
        assert_eq!(
            tiff_plan.known_required_bytes - plan.known_required_bytes,
            734_000_000
        );
        assert_eq!(tiff_plan.required_bytes - plan.required_bytes, 734_000_000);
        assert_eq!(
            plan.known_required_bytes,
            film::STORAGE + plan.gamut.scratch_bytes
        );
        let mut changed = plan.clone();
        changed.missing.pop();
        assert!(changed.validate(&fixture).is_err());
        let mut changed = plan.clone();
        changed.storage_reserve_bytes -= 1;
        assert!(changed.validate(&fixture).is_err());
        let mut changed = plan.clone();
        changed.required_bytes -= 1;
        assert!(changed.validate(&fixture).is_err());
        let mut changed = plan.clone();
        changed.jpeg.batch_pixels += 1;
        assert!(changed.validate(&fixture).is_err());
    }
    #[test]
    fn exact_fit_admits_but_unknown_outside_and_insufficient_budget_are_distinct() {
        let mut docs = documents();
        let fixture = docs.catalogue.fixtures[0].clone();
        docs.envelope
            .validate(&docs.catalogue, &docs.envelope.catalogue_sha256)
            .unwrap();
        let reserve = 1 << 20;
        docs.envelope.cases = vec![case(&fixture, (8 << 30) - film::STORAGE - reserve, reserve)];
        let (_, plan) = docs.plan(&fixture.id, &"7".repeat(64), 8 << 30).unwrap();
        assert_eq!(plan.required_bytes, 8 << 30);
        assert_eq!(
            docs.plan(&fixture.id, &"7".repeat(64), (8 << 30) - 1),
            Err(ErrorCode::ResourceBudget)
        );
        docs.envelope.cases = vec![Case::Unknown {
            fixture_id: fixture.id.clone(),
        }];
        assert_eq!(
            docs.plan(&fixture.id, &"7".repeat(64), 8 << 30),
            Err(ErrorCode::UnqualifiedEnvelope)
        );
        docs.envelope.cases.clear();
        assert_eq!(
            docs.plan(&fixture.id, &"7".repeat(64), 8 << 30),
            Err(ErrorCode::OutsideEnvelope)
        );
        assert_eq!(
            docs.plan(&"9".repeat(32), &"7".repeat(64), 8 << 30),
            Err(ErrorCode::UnknownFixture)
        );
    }
    #[test]
    fn scope_and_environment_identity_do_not_expand_from_geometry_or_a_hash_alone() {
        let mut docs = documents();
        docs.envelope.cases.push(docs.envelope.cases[0].clone());
        assert!(
            docs.envelope
                .validate(&docs.catalogue, &docs.envelope.catalogue_sha256)
                .is_err()
        );
        docs.envelope.cases.pop();
        let mut foreign = docs.catalogue.fixtures[0].clone();
        foreign.id = "9".repeat(32);
        std::mem::swap(&mut foreign.width, &mut foreign.height);
        docs.envelope.cases.push(case(&foreign, 1 << 30, 1 << 20));
        assert!(
            docs.envelope
                .validate(&docs.catalogue, &docs.envelope.catalogue_sha256)
                .is_err()
        );
        let e = &docs.envelope;
        e.matches(&e.image, &e.launcher_sha256, &e.environment)
            .unwrap();
        assert_eq!(
            e.matches(
                &format!("sha256:{}", "a".repeat(64)),
                &e.launcher_sha256,
                &e.environment
            ),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(
            e.matches(&e.image, &"b".repeat(64), &e.environment),
            Err(ErrorCode::Unavailable)
        );
        let mut changed = e.environment.clone();
        changed.cpu_sha256 = "c".repeat(64);
        assert_eq!(
            e.matches(&e.image, &e.launcher_sha256, &changed),
            Err(ErrorCode::Unavailable)
        );
    }
}
