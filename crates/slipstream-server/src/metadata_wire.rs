//! Shared Read/Save Metadata JSON contract for HTTP and CLI consumers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataReadResult {
    pub photo_id: String,
    pub original_location: String,
    pub association: MetadataAssociation,
    pub fields: BTreeMap<String, MetadataField>,
    pub capture_facts: BTreeMap<String, MetadataCaptureFact>,
    pub library_rating: u8,
    pub evidence: MetadataEvidence,
    pub save_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataAssociation {
    pub state: MetadataAssociationState,
    pub candidates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataAssociationState {
    Eligible,
    Ambiguous,
    Unresolved,
    Ineligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataFieldState {
    Present,
    Absent,
    Invalid,
    Unavailable,
    ResourceLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataProvenance {
    Sidecar,
    EmbeddedXmp,
    IptcIim,
    Original,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    Text(String),
    Number(serde_json::Number),
    Boolean(bool),
    List(Vec<String>),
    Languages(BTreeMap<String, String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataSourceValue {
    pub state: MetadataFieldState,
    pub provenance: MetadataProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<MetadataValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_alternatives_available: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataField {
    pub state: MetadataFieldState,
    pub writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MetadataProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<MetadataValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_value: Option<MetadataValue>,
    pub sources: Vec<MetadataSourceValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataCaptureFact {
    pub state: MetadataFieldState,
    pub identifier: String,
    pub unit: String,
    pub provenance: MetadataProvenance,
    pub writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<MetadataValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataFileFacts {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetadataSidecarEvidence {
    Absent,
    Present {
        location: String,
        facts: MetadataFileFacts,
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataEvidence {
    pub photo_id: String,
    pub original_location: String,
    pub original: MetadataFileFacts,
    pub sidecar: MetadataSidecarEvidence,
    pub association_generation: u64,
    pub instance_epoch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataSaveRequest {
    pub evidence: MetadataEvidence,
    pub changes: BTreeMap<String, MetadataChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
pub enum MetadataChange {
    #[serde(rename = "set")]
    Set { value: MetadataValue },
    #[serde(rename = "clear")]
    Clear,
    #[serde(rename = "remove")]
    Remove,
    /// Null removes only the named alternative; unmentioned languages survive.
    #[serde(rename = "setLanguages")]
    SetLanguages {
        languages: BTreeMap<String, Option<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetadataSaveResult {
    pub photo_id: String,
    pub sidecar_location: String,
    pub affected_fields: Vec<String>,
    pub verified_values: BTreeMap<String, MetadataField>,
    pub evidence: MetadataEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataErrorCode {
    InvalidInput,
    UnsupportedField,
    PhotoMissing,
    OriginalUnavailable,
    AssociationUnresolved,
    PhotoRemoved,
    MetadataMalformed,
    EvidenceStale,
    SaveUnavailable,
    Permission,
    ResourceLimit,
    StorageFailure,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataErrorEnvelope {
    pub error: MetadataError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataError {
    pub code: MetadataErrorCode,
    pub message: String,
    pub details: serde_json::Value,
}

pub fn http_status(code: MetadataErrorCode) -> u16 {
    use MetadataErrorCode::*;
    match code {
        InvalidInput | UnsupportedField => 400,
        Permission => 403,
        PhotoMissing | OriginalUnavailable => 404,
        PhotoRemoved | EvidenceStale | AssociationUnresolved => 409,
        ResourceLimit => 413,
        MetadataMalformed => 422,
        SaveUnavailable => 501,
        StorageFailure | OutcomeUnknown => 500,
    }
}

pub fn cli_exit(code: MetadataErrorCode) -> i32 {
    use MetadataErrorCode::*;
    match code {
        InvalidInput | UnsupportedField => 2,
        PhotoMissing | OriginalUnavailable | AssociationUnresolved | PhotoRemoved => 3,
        EvidenceStale | MetadataMalformed => 4,
        SaveUnavailable | Permission => 5,
        ResourceLimit => 6,
        StorageFailure | OutcomeUnknown => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const READ: &str = include_str!("../../../compatibility/metadata/external-metadata-read.json");
    const SAVE: &str = include_str!("../../../compatibility/metadata/external-metadata-save.json");

    fn round_trip<T: serde::de::DeserializeOwned + Serialize>(value: &Value, name: &str) {
        let decoded: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *value, "{name}");
    }

    #[test]
    fn read_vectors_round_trip() {
        let cases: Vec<Value> = serde_json::from_str(READ).unwrap();
        for case in cases {
            round_trip::<MetadataReadResult>(&case["result"], case["name"].as_str().unwrap());
        }
    }

    #[test]
    fn save_vectors_round_trip_and_map_every_error() {
        let cases: Vec<Value> = serde_json::from_str(SAVE).unwrap();
        let mut codes = std::collections::BTreeSet::new();
        for case in cases {
            let name = case["name"].as_str().unwrap();
            match case["kind"].as_str().unwrap() {
                "request" => round_trip::<MetadataSaveRequest>(&case["request"], name),
                "result" => round_trip::<MetadataSaveResult>(&case["result"], name),
                "error" => {
                    round_trip::<MetadataErrorEnvelope>(&case["response"], name);
                    let envelope: MetadataErrorEnvelope =
                        serde_json::from_value(case["response"].clone()).unwrap();
                    let code = envelope.error.code;
                    assert_eq!(
                        u64::from(http_status(code)),
                        case["httpStatus"].as_u64().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        i64::from(cli_exit(code)),
                        case["cliExit"].as_i64().unwrap(),
                        "{name}"
                    );
                    assert!(
                        codes.insert(
                            case["response"]["error"]["code"]
                                .as_str()
                                .unwrap()
                                .to_owned()
                        )
                    );
                }
                kind => panic!("unknown vector kind: {kind}"),
            }
        }
        assert_eq!(codes.len(), 13);
    }
}
