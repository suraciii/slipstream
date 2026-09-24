use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, time::SystemTime};

pub const REQUEST_BYTES: usize = 16 * 1024;
pub const RESPONSE_BYTES: usize = 64 * 1024;
pub const TERMINAL_SNAPSHOT_BYTES: usize = 4 * 1024;
pub const STORAGE_BYTES: u64 = 16 * 1024 * 1024;
pub const PROFILE: &str = "slipstream-native-qualification-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
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
}

impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.len() > REQUEST_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ErrorCode> {
        let image_digest = self.image.strip_prefix("sha256:").or_else(|| {
            let (repository, digest) = self.image.split_once("@sha256:")?;
            (!repository.is_empty()
                && repository.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/._-:".contains(&byte)
                }))
            .then_some(digest)
        });
        if self.version != 1
            || self.mode != "qualification"
            || !hex(&self.instance, 32)
            || !Path::new(&self.root).is_absolute()
            || !Path::new(&self.socket).is_absolute()
            || self
                .root
                .bytes()
                .any(|byte| byte <= 0x20 || b",\\\"".contains(&byte))
            || self.root.chars().any(char::is_whitespace)
            || self.socket.contains('\0')
            || self.peer_uid == u32::MAX
            || !image_digest.is_some_and(|digest| hex(digest, 64))
            || !(64 * 1024 * 1024..=256 * 1024 * 1024).contains(&self.memory_bytes)
            || !self.memory_bytes.is_multiple_of(4096)
            || !(1..=604800).contains(&self.receipt_retention_seconds)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }

    pub(crate) fn limits(&self) -> Limits {
        if matches!(self.version, 2 | 3) {
            crate::film::limits(self.memory_bytes)
        } else {
            Limits::new(self.memory_bytes)
        }
    }

    pub(crate) fn policy(&self) -> String {
        digest(&serde_json::to_vec(&(PROFILE, self)).expect("serializable configuration"))
    }

    pub(crate) fn parent_unit(&self) -> String {
        format!("slipstreamprocessing{}.slice", self.instance)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Workload {
    ProbeSuccess,
    ProbeNativeOom,
    ProbeDescendantOom,
    #[serde(rename = "probe-exit-137")]
    ProbeExit137,
    ProbeHold,
    ProbeStorageFull,
    ProbeInodesFull,
    #[serde(untagged)]
    Film(crate::film::Workload),
}

impl Workload {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ProbeSuccess => "probe-success",
            Self::ProbeNativeOom => "probe-native-oom",
            Self::ProbeDescendantOom => "probe-descendant-oom",
            Self::ProbeExit137 => "probe-exit-137",
            Self::ProbeHold => "probe-hold",
            Self::ProbeStorageFull => "probe-storage-full",
            Self::ProbeInodesFull => "probe-inodes-full",
            Self::Film(_) => "film-fixture",
        }
    }
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
    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.len() > REQUEST_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn instance(&self) -> &str {
        match self {
            Self::Reconcile { instance, .. }
            | Self::Start { instance, .. }
            | Self::Inspect { instance, .. }
            | Self::Cancel { instance, .. } => instance,
        }
    }

    fn validate(&self) -> Result<(), ErrorCode> {
        let version = match self {
            Self::Reconcile { version, .. }
            | Self::Start { version, .. }
            | Self::Inspect { version, .. }
            | Self::Cancel { version, .. } => *version,
        };
        let identity_valid = match self {
            Self::Reconcile { .. } => true,
            Self::Start {
                incarnation,
                sequence,
                policy,
                bundle,
                ..
            } => hex(incarnation, 32) && *sequence > 0 && hex(policy, 64) && hex(bundle, 64),
            Self::Inspect {
                incarnation,
                sequence,
                ..
            }
            | Self::Cancel {
                incarnation,
                sequence,
                ..
            } => hex(incarnation, 32) && *sequence > 0,
        };
        if version != 1
            || !hex(self.instance(), 32)
            || !identity_valid
            || matches!(
                self,
                Self::Start {
                    workload: Workload::Film(_),
                    ..
                }
            )
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthorized,
    WrongInstance,
    Unavailable,
    IncompatiblePolicy,
    IncompatibleBundle,
    Conflict,
    Busy,
    Expired,
    UnknownAttempt,
    StaleIncarnation,
    Capacity,
    Uncertain,
    IncompatibleCatalogue,
    IncompatibleResourceModel,
    IncompatibleEnvelope,
    OutsideEnvelope,
    UnqualifiedEnvelope,
    ResourceBudget,
    UnknownFixture,
    UnsupportedFixture,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub code: ErrorCode,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Response {
    Result {
        version: u8,
        result: Box<ResultBody>,
    },
    Error {
        version: u8,
        error: ErrorResponse,
    },
}

impl From<Result<ResultBody, ErrorCode>> for Response {
    fn from(result: Result<ResultBody, ErrorCode>) -> Self {
        match result {
            Ok(result) => Self::Result {
                version: 1,
                result: Box::new(result),
            },
            Err(code) => Self::Error {
                version: 1,
                error: ErrorResponse { code },
            },
        }
    }
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
        availability: Availability,
        active: Option<Receipt>,
    },
    Receipt {
        receipt: Receipt,
    },
    #[serde(untagged)]
    Film(Box<crate::film::ResultBody>),
    #[serde(untagged)]
    Qualified(Box<crate::qualified::ResultBody>),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Accepted,
    Running,
    Settling,
    Settled,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Completed,
    AllocationFailed,
    Oom,
    StorageFull,
    Cancelled,
    Deadline,
    EngineFailed,
    Interrupted,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Cleanup {
    Pending,
    Complete,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub launch_id: String,
    pub container_id: Option<String>,
    pub attempt_unit: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub memory_bytes: u64,
    pub swap_bytes: u64,
    pub cpu_quota_us: u64,
    pub cpu_period_us: u64,
    pub tasks: u64,
    pub storage_bytes: u64,
    pub storage_inodes: u64,
}

impl Limits {
    pub(crate) fn new(memory_bytes: u64) -> Self {
        Self {
            memory_bytes,
            swap_bytes: 0,
            cpu_quota_us: 100000,
            cpu_period_us: 100000,
            tasks: 32,
            storage_bytes: STORAGE_BYTES,
            storage_inodes: 64,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Events {
    pub oom: u64,
    pub oom_kill: u64,
    pub oom_group_kill: u64,
    pub local_oom: u64,
    pub local_oom_kill: u64,
    pub local_oom_group_kill: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalSnapshot {
    pub cgroup_path: String,
    pub cgroup_inode: u64,
    pub unit_invocation: String,
    pub launch_id: String,
    pub container_id: String,
    pub attempt_unit: String,
    pub incarnation: String,
    pub sequence: u64,
    pub memory_peak_raw: String,
    pub memory_max_raw: String,
    pub memory_swap_current_raw: String,
    pub memory_swap_max_raw: String,
    pub memory_events_raw: String,
    pub memory_events_local_raw: String,
    #[serde(default)]
    pub io_stat_raw: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub peak_bytes: u64,
    pub exit_code: Option<u8>,
    pub docker_oom_killed: Option<bool>,
    pub attempt_before: Option<Events>,
    pub attempt_after: Option<Events>,
    pub parent_before: Option<Events>,
    pub parent_after: Option<Events>,
    pub populated: Option<bool>,
    pub terminal_snapshot: Option<TerminalSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub incarnation: String,
    pub sequence: u64,
    pub workload: Workload,
    pub policy: String,
    pub bundle: String,
    pub state: State,
    pub cancellation_requested: bool,
    pub accepted_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub outcome: Option<Outcome>,
    pub runtime: Option<Runtime>,
    pub limits: Limits,
    pub evidence: Option<Evidence>,
    pub cleanup: Cleanup,
}

pub(crate) fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn now() -> Result<u64, ErrorCode> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or(ErrorCode::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn config() -> Config {
        Config {
            version: 1,
            mode: "qualification".into(),
            instance: "0".repeat(32),
            root: "/var/lib/processing/test".into(),
            socket: "/run/processing/test.sock".into(),
            peer_uid: 1000,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 128 * 1024 * 1024,
            receipt_retention_seconds: 86400,
        }
    }

    #[test]
    fn documented_examples_parse_as_written() {
        let source = include_str!("../../../design/processing-executor-protocol.md");
        let mut count = 0;
        for block in source.split("```json\n").skip(1) {
            let bytes = block.split("```").next().unwrap().as_bytes();
            if serde_json::from_slice::<serde_json::Value>(bytes)
                .unwrap()
                .get("op")
                .is_some()
            {
                Request::parse(bytes).unwrap();
            } else {
                Config::parse(bytes).unwrap();
            }
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn every_closed_workload_uses_the_documented_wire_name() {
        for workload in [
            Workload::ProbeSuccess,
            Workload::ProbeNativeOom,
            Workload::ProbeDescendantOom,
            Workload::ProbeExit137,
            Workload::ProbeHold,
            Workload::ProbeStorageFull,
            Workload::ProbeInodesFull,
        ] {
            assert_eq!(serde_json::to_value(&workload).unwrap(), workload.name());
            assert_eq!(
                serde_json::from_value::<Workload>(serde_json::json!(workload.name())).unwrap(),
                workload
            );
        }
        assert!(serde_json::from_str::<Workload>("\"python\"").is_err());
    }

    #[test]
    fn requests_reject_duplicate_unknown_trailing_and_noninteger_fields() {
        let base = format!(
            r#"{{"op":"inspect","version":1,"instance":"{}","incarnation":"{}","sequence":1}}"#,
            "0".repeat(32),
            "1".repeat(32)
        );
        assert!(Request::parse(base.as_bytes()).is_ok());
        for value in [
            "-1",
            "-0",
            "1.0",
            "true",
            "18446744073709551616",
            "0",
            "null",
            "\"1\"",
        ] {
            assert!(
                Request::parse(
                    base.replace("\"sequence\":1", &format!("\"sequence\":{value}"))
                        .as_bytes()
                )
                .is_err(),
                "{value}"
            );
        }
        for changed in [
            base.replace("\"version\":1", "\"version\":1,\"version\":1"),
            base.replace("\"version\":1", "\"version\":1,\"argv\":[]"),
            format!("{base}{{}}"),
            base.replace("\"version\":1", "\"version\":2"),
        ] {
            assert!(Request::parse(changed.as_bytes()).is_err());
        }
        assert!(Request::parse(&vec![b' '; REQUEST_BYTES + 1]).is_err());
    }

    #[test]
    fn policy_enforces_exact_image_and_bounded_resources() {
        let original = config();
        original.validate().unwrap();
        for memory in [
            0,
            63 * 1024 * 1024,
            64 * 1024 * 1024 + 1,
            257 * 1024 * 1024,
            u64::MAX,
        ] {
            let mut value = original.clone();
            value.memory_bytes = memory;
            assert!(value.validate().is_err());
        }
        for image in [
            "image:latest".into(),
            format!("sha256:{}", "A".repeat(64)),
            format!("@sha256:{}", "1".repeat(64)),
        ] {
            let mut value = original.clone();
            value.image = image;
            assert!(value.validate().is_err());
        }
        for root in [
            "/var/lib/a,b",
            "/var/lib/a\\b",
            "/var/lib/a\nb",
            "/var/lib/a\u{2003}b",
            "relative",
        ] {
            let mut value = original.clone();
            value.root = root.into();
            assert!(value.validate().is_err());
        }
        for retention in [0, 604801, u64::MAX] {
            let mut value = original.clone();
            value.receipt_retention_seconds = retention;
            assert!(value.validate().is_err());
        }
        let mut changed = original.clone();
        changed.memory_bytes = 64 * 1024 * 1024;
        assert_ne!(original.policy(), changed.policy());
        let bytes = serde_json::to_vec(&original).unwrap();
        Config::parse(&bytes).unwrap();
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":1", "\"version\":1,\"version\":1");
        assert!(Config::parse(text.as_bytes()).is_err());
    }
}
