use super::*;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Envelope {
    pub(crate) schema_version: u8,
    pub(crate) status: &'static str,
    pub(crate) data: Option<Value>,
    pub(crate) error: Option<ErrorPayload>,
}

impl Envelope {
    pub(crate) fn success(data: Value) -> Self {
        Self {
            schema_version: 1,
            status: "ok",
            data: Some(data),
            error: None,
        }
    }

    pub(crate) fn error(error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "error",
            data: None,
            error: Some(error),
        }
    }

    /// A mixed Photo batch keeps its confirmed results in `data` beside the
    /// `partial_result` error.
    pub(crate) fn partial(data: Value, error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "partial",
            data: Some(data),
            error: Some(error),
        }
    }

    /// An all-unsuccessful Photo batch keeps its complete result array in
    /// `data` beside its partition error.
    pub(crate) fn error_with_data(data: Value, error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "error",
            data: Some(data),
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ErrorPayload {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) effect: String,
    pub(crate) details: Value,
}

#[derive(Debug)]
pub(crate) struct CommandFailure {
    pub(crate) exit_code: u8,
    pub(crate) payload: ErrorPayload,
    /// Confirmed per-Photo results for a mixed or all-unsuccessful Photo
    /// batch; the complete result array stays in `data` for those failures.
    pub(crate) data: Option<Box<Value>>,
}

impl CommandFailure {
    pub(crate) fn from_payload(exit_code: u8, payload: ErrorPayload) -> Self {
        Self {
            exit_code,
            payload,
            data: None,
        }
    }

    pub(crate) fn invalid(argument: &str, reason: impl Into<String>) -> Self {
        Self::from_payload(
            2,
            ErrorPayload {
                code: "invalid_input".to_owned(),
                message: "Correct the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "argument": argument, "reason": reason.into() }),
            },
        )
    }

    pub(crate) fn transport(operation: Operation) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "transport_failed".to_owned(),
                message: "Check the service connection and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        )
    }

    pub(crate) fn incompatible(supported: Vec<u16>) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "incompatible_server".to_owned(),
                message: "Use a compatible Slipstream client and service.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "requestedContractVersion": CLI_CONTRACT_VERSION,
                    "supportedContractVersions": supported,
                }),
            },
        )
    }

    pub(crate) fn limit_exceeded(limit_name: &str, limit: usize, actual: usize) -> Self {
        Self::from_payload(
            2,
            ErrorPayload {
                code: "limit_exceeded".to_owned(),
                message: "Reduce the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "limitName": limit_name, "limit": limit, "actual": actual }),
            },
        )
    }

    pub(crate) fn local_input(path: Option<&str>) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "Check the local input file and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": "read-input",
                    "path": path,
                    "fileCommitted": false,
                }),
            },
        )
    }

    pub(crate) fn published_file(data: Value, interrupted: bool, noun: &str) -> Self {
        Self::from_payload(
            if interrupted { 130 } else { 6 },
            ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: format!("The {noun} file was published; inspect it before trying again."),
                effect: "partial".to_owned(),
                details: json!({
                    "operation": "write-output",
                    "path": data["path"],
                    "fileCommitted": true,
                }),
            },
        )
        .with_data(data)
    }

    pub(crate) fn local_credential(path: Option<&str>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "Check the local credential file and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": "read-credential",
                    "path": path,
                    "fileCommitted": false,
                }),
            },
            data: None,
        }
    }

    pub(crate) fn authentication_required(operation: Operation) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "authentication_required".to_owned(),
                message: "Provide a valid Access Token and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
            data: None,
        }
    }

    pub(crate) fn access_denied(operation: Operation) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "access_denied".to_owned(),
                message: "The service denied access to this operation.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
            data: None,
        }
    }

    pub(crate) fn server_busy(operation: Operation, retry_after_seconds: Option<u64>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "server_busy".to_owned(),
                message: "The service is temporarily unavailable. Try again later.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": operation.wire(),
                    "retryAfterSeconds": retry_after_seconds,
                }),
            },
            data: None,
        }
    }

    pub(crate) fn unknown(identity: &MutationIdentity) -> Self {
        Self::from_payload(
            7,
            ErrorPayload {
                code: "outcome_unknown".to_owned(),
                message: "Inspect the current state with a read command before continuing."
                    .to_owned(),
                effect: "unknown".to_owned(),
                details: json!({
                    "operation": identity.operation.wire(),
                    "photoIds": identity.photo_ids,
                    "albumId": identity.album_id,
                    "albumName": identity.album_name,
                }),
            },
        )
    }

    pub(crate) fn interrupted_unknown(identity: &MutationIdentity) -> Self {
        Self::from_payload(
            130,
            ErrorPayload {
                code: "outcome_unknown".to_owned(),
                message: "The command was interrupted and the outcome is unknown. Inspect the current state before continuing.".to_owned(),
                effect: "unknown".to_owned(),
                details: json!({
                    "operation": identity.operation.wire(),
                    "photoIds": identity.photo_ids,
                    "albumId": identity.album_id,
                    "albumName": identity.album_name,
                }),
            },
        )
    }

    /// A mixed Photo batch: at least one sibling decision committed while
    /// at least one requested Photo conflicted or was missing.
    pub(crate) fn photo_batch_partial(counts: &Value) -> Self {
        Self::from_payload(
            5,
            ErrorPayload {
                code: "partial_result".to_owned(),
                message: "Read the confirmed sibling outcomes, then re-check each conflicting or missing Photo with a fresh version.".to_owned(),
                effect: "partial".to_owned(),
                details: json!({ "counts": counts }),
            },
        )
    }

    /// A batch whose every item conflicted; the details identify the first
    /// conflicting Photo in request order.
    pub(crate) fn photo_batch_conflict(reference: &str, current_version: &str) -> Self {
        Self::from_payload(
            4,
            ErrorPayload {
                code: "conflict".to_owned(),
                message: "Read the current decisions and retry with their fresh versions."
                    .to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "resource": "photo",
                    "reference": reference,
                    "currentVersion": current_version,
                }),
            },
        )
    }

    /// A batch whose every item was missing; the details identify the first
    /// missing Photo in request order.
    pub(crate) fn photo_batch_missing(reference: &str) -> Self {
        Self::from_payload(
            3,
            ErrorPayload {
                code: "not_found".to_owned(),
                message: "Check the requested Photo IDs against a fresh query.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "resource": "photo", "reference": reference }),
            },
        )
    }

    pub(crate) fn with_data(mut self, data: Value) -> Self {
        self.data = Some(Box::new(data));
        self
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error: ErrorPayload,
}

// The Read and Save Metadata wire contract mirrors the server's shared
// `metadata_wire` shapes, which the JSON vectors under
// `compatibility/metadata/` pin. The CLI validates envelopes locally and
// never re-derives field semantics.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataReadWire {
    pub(crate) photo_id: String,
    pub(crate) original_location: String,
    pub(crate) association: MetadataAssociationWire,
    pub(crate) fields: std::collections::BTreeMap<String, MetadataFieldWire>,
    pub(crate) capture_facts: std::collections::BTreeMap<String, MetadataCaptureFactWire>,
    pub(crate) library_rating: u8,
    pub(crate) evidence: MetadataEvidenceWire,
    pub(crate) save_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) save_unavailable_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataAssociationWire {
    pub(crate) state: MetadataAssociationStateWire,
    pub(crate) candidates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetadataAssociationStateWire {
    Eligible,
    Ambiguous,
    Unresolved,
    Ineligible,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetadataFieldStateWire {
    Present,
    Absent,
    Invalid,
    Unavailable,
    ResourceLimit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MetadataProvenanceWire {
    Sidecar,
    EmbeddedXmp,
    IptcIim,
    Original,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum MetadataValueWire {
    Text(String),
    Number(serde_json::Number),
    Boolean(bool),
    List(Vec<String>),
    Languages(std::collections::BTreeMap<String, String>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataSourceValueWire {
    pub(crate) state: MetadataFieldStateWire,
    pub(crate) provenance: MetadataProvenanceWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<MetadataValueWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) language_alternatives_available: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataFieldWire {
    pub(crate) state: MetadataFieldStateWire,
    pub(crate) writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provenance: Option<MetadataProvenanceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<MetadataValueWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) inferred_value: Option<MetadataValueWire>,
    pub(crate) sources: Vec<MetadataSourceValueWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataCaptureFactWire {
    pub(crate) state: MetadataFieldStateWire,
    pub(crate) identifier: String,
    pub(crate) unit: String,
    pub(crate) provenance: MetadataProvenanceWire,
    pub(crate) writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<MetadataValueWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) problem: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataSaveResultWire {
    pub(crate) photo_id: String,
    pub(crate) sidecar_location: String,
    pub(crate) affected_fields: Vec<String>,
    pub(crate) verified_values: std::collections::BTreeMap<String, MetadataFieldWire>,
    pub(crate) evidence: MetadataEvidenceWire,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataEvidenceWire {
    pub(crate) photo_id: String,
    pub(crate) original_location: String,
    pub(crate) original: MetadataFileFactsWire,
    pub(crate) sidecar: MetadataSidecarEvidenceWire,
    pub(crate) association_generation: u64,
    pub(crate) instance_epoch: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MetadataFileFactsWire {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) size: u64,
    pub(crate) modified_seconds: i64,
    pub(crate) modified_nanoseconds: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MetadataSidecarEvidenceWire {
    Absent,
    Unavailable {
        reason: String,
    },
    Present {
        location: String,
        facts: MetadataFileFactsWire,
        sha256: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataErrorEnvelopeWire {
    pub(crate) error: MetadataErrorWire,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataErrorWire {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) details: Value,
}

/// Maps a metadata error code to the shared exit-code table. The server owns
/// the table; this mirror must stay identical to
/// `slipstream-server::metadata_wire::cli_exit`.
pub(crate) fn metadata_failure(error: MetadataErrorWire) -> CommandFailure {
    let exit_code = match error.code.as_str() {
        "invalid_input" | "unsupported_field" => 2,
        "photo_missing" | "original_unavailable" | "association_unresolved" | "photo_removed" => 3,
        "evidence_stale" | "metadata_malformed" => 4,
        "save_unavailable" | "permission" => 5,
        "resource_limit" => 6,
        "storage_failure" | "outcome_unknown" => 7,
        _ => 6,
    };
    let effect = if error.code == "outcome_unknown" {
        "unknown"
    } else {
        "none"
    };
    CommandFailure::from_payload(
        exit_code,
        ErrorPayload {
            code: error.code,
            message: error.message,
            effect: effect.to_owned(),
            details: error.details,
        },
    )
}

pub(crate) fn retry_after_seconds(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Maps only the access-boundary statuses whose refusal semantics are part of
/// the CLI contract. A 503 is trusted only for the explicit access errors.
pub(crate) fn access_boundary_failure(
    status: StatusCode,
    retry_after: Option<u64>,
    body: &[u8],
    operation: Operation,
) -> Option<CommandFailure> {
    match status {
        StatusCode::UNAUTHORIZED => Some(CommandFailure::authentication_required(operation)),
        StatusCode::FORBIDDEN => Some(CommandFailure::access_denied(operation)),
        StatusCode::TOO_MANY_REQUESTS => Some(CommandFailure::server_busy(operation, retry_after)),
        StatusCode::SERVICE_UNAVAILABLE if is_access_unavailable(body) => {
            Some(CommandFailure::server_busy(operation, None))
        }
        _ => None,
    }
}

pub(crate) fn is_access_unavailable(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1
        && matches!(
            object.get("error").and_then(Value::as_str),
            Some("access_unavailable" | "access_unconfigured")
        )
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusData {
    #[serde(skip_deserializing, default = "client_version")]
    pub(crate) client_version: String,
    pub(crate) server_version: String,
    pub(crate) cli_contract_version: u16,
    pub(crate) published: bool,
    pub(crate) publication: Option<String>,
    pub(crate) photo_count: u64,
    pub(crate) scan: ScanStatus,
}

pub(crate) fn client_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScanStatus {
    pub(crate) state: ScanState,
    pub(crate) publication: Option<String>,
    pub(crate) completed: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) last_recovery: Option<RecoveryCounts>,
    pub(crate) fingerprints: Option<FingerprintCounts>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ScanState {
    Initializing,
    Discovering,
    Inspecting,
    Recovering,
    Applying,
    Idle,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryCounts {
    pub(crate) relocated_photos: u64,
    pub(crate) fingerprinted_originals: u64,
    pub(crate) unavailable_photos: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FingerprintCounts {
    pub(crate) enrolled: u64,
    pub(crate) pending: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderListData {
    pub(crate) items: Vec<FolderItem>,
    pub(crate) total: u64,
    pub(crate) next_cursor: Option<String>,
    pub(crate) evaluated_at: String,
    pub(crate) expires_at: Option<String>,
    pub(crate) publication: String,
    pub(crate) parent: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderItem {
    pub(crate) location: String,
    pub(crate) name: String,
    pub(crate) photo_count: u64,
    pub(crate) has_descendant_folders: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListData<T> {
    pub(crate) items: Vec<T>,
    pub(crate) total: u64,
    pub(crate) next_cursor: Option<String>,
    pub(crate) evaluated_at: String,
    pub(crate) expires_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum AlbumListItem {
    Missing(MissingItem),
    Present(AlbumSummary),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MissingItem {
    pub(crate) id: String,
    pub(crate) state: MissingState,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MissingState {
    Missing,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbumSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) photo_count: u64,
    pub(crate) has_saved_position: bool,
    pub(crate) album_version: String,
    #[serde(rename = "webPath")]
    pub(crate) web_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum PhotoListItem {
    Missing(MissingItem),
    Present(PhotoItem),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhotoItem {
    pub(crate) id: String,
    pub(crate) filename: String,
    pub(crate) original_kind: OriginalKind,
    pub(crate) original_available: bool,
    pub(crate) selection_state: SelectionState,
    pub(crate) rating: u8,
    pub(crate) decision_version: String,
    pub(crate) removed_at_ms: Option<i64>,
    pub(crate) capture_time: Option<String>,
    pub(crate) preview: PreviewFacts,
    #[serde(rename = "webPath")]
    pub(crate) web_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OriginalKind {
    Raw,
    Jpeg,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SelectionState {
    Undecided,
    Selected,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PreviewState {
    InspectionPending,
    Ready,
    Failed,
    Unavailable,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewFacts {
    pub(crate) state: PreviewState,
    pub(crate) source: Option<PreviewSource>,
    pub(crate) source_revision: Option<String>,
    pub(crate) width: Option<u64>,
    pub(crate) height: Option<u64>,
    pub(crate) detail_limited: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PreviewSource {
    JpegOriginal,
    RawEmbeddedJpeg,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhotoGet {
    pub(crate) id: String,
    pub(crate) filename: String,
    pub(crate) original_kind: OriginalKind,
    pub(crate) original_available: bool,
    pub(crate) selection_state: SelectionState,
    pub(crate) rating: u8,
    pub(crate) decision_version: String,
    pub(crate) removed_at_ms: Option<i64>,
    pub(crate) capture_time: Option<String>,
    pub(crate) preview: PreviewFacts,
    #[serde(rename = "webPath")]
    pub(crate) web_path: String,
    pub(crate) metadata: PhotoMetadata,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhotoMetadata {
    pub(crate) state: MetadataState,
    pub(crate) capture_time: Option<String>,
    pub(crate) aperture: Option<String>,
    pub(crate) shutter_speed: Option<String>,
    pub(crate) focal_length: Option<String>,
    pub(crate) iso: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MetadataState {
    Pending,
    Known,
    Missing,
    Invalid,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhotoQueryRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<PhotoSource<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selection: Option<SelectionArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rating_minimum: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rating_maximum: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<OriginalKindArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) captured_from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) captured_before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) order: Option<OrderArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) limit: Option<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum PhotoSource<'a> {
    Album {
        #[serde(rename = "albumId")]
        album_id: &'a str,
    },
    Folder {
        location: &'a str,
    },
}

/// The one accepted `--input` document shape. Deserializing it rejects
/// unknown keys, duplicate keys, trailing content, and non-object documents.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MembershipInput {
    pub(crate) photo_ids: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemovalInput {
    pub(crate) photos: Vec<RemovalInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemovalInputPhoto {
    pub(crate) photo_id: String,
    pub(crate) selection_state: String,
    pub(crate) decision_version: String,
    pub(crate) removed_at_ms: Value,
}

#[derive(Debug)]
pub(crate) struct PreparedRemoval {
    pub(crate) photos: Vec<RemovalTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct RemovalTarget {
    pub(crate) photo_id: String,
    pub(crate) selection_state: String,
    pub(crate) decision_version: String,
    pub(crate) removed_at_ms: Option<i64>,
}

impl PreparedRemoval {
    pub(crate) fn body(&self, operation_id: &str) -> Value {
        json!({
            "operationId": operation_id,
            "photos": self.photos.iter().map(|photo| json!({
                "photoId": photo.photo_id,
                "selectionState": photo.selection_state,
                "decisionVersion": photo.decision_version,
                "removedAtMs": photo.removed_at_ms,
            })).collect::<Vec<_>>(),
        })
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreInput {
    pub(crate) photos: Vec<RestoreInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreInputPhoto {
    pub(crate) photo_id: String,
    pub(crate) removed_at_ms: i64,
}

#[derive(Debug)]
pub(crate) struct PreparedRestore {
    pub(crate) markers: Vec<RestoreMarker>,
}

#[derive(Clone, Debug)]
pub(crate) struct RestoreMarker {
    pub(crate) photo_id: String,
    pub(crate) removed_at_ms: i64,
}

impl PreparedRestore {
    pub(crate) fn body(&self, operation_id: &str) -> Value {
        json!({
            "operationId": operation_id,
            "photos": self.markers.iter().map(|marker| json!({
                "photoId": marker.photo_id,
                "removedAtMs": marker.removed_at_ms,
            })).collect::<Vec<_>>(),
        })
    }
}

/// One identified Photo together with the decision version the caller
/// observed before intending to write.
#[derive(Clone, Debug)]
pub(crate) struct DecisionTarget {
    pub(crate) photo_id: String,
    pub(crate) if_version: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DecisionField {
    SelectionState,
    Rating,
}

impl DecisionField {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::SelectionState => "selectionState",
            Self::Rating => "rating",
        }
    }
}

/// One validated decision request shared by the single-Photo and batch forms.
/// A single-Photo command is a one-item batch.
#[derive(Debug)]
pub(crate) struct PreparedDecision {
    pub(crate) field: DecisionField,
    pub(crate) value: Value,
    pub(crate) photos: Vec<DecisionTarget>,
}

impl PreparedDecision {
    pub(crate) fn body(&self) -> Value {
        json!({
            "field": self.field.wire(),
            "value": self.value,
            "photos": self.photos.iter().map(|photo| json!({
                "photoId": photo.photo_id,
                "ifVersion": photo.if_version,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The one accepted `photos set --input` document shape. Deserializing it
/// rejects unknown keys, duplicate keys, trailing content, and non-object
/// documents, including inside each Photo item.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DecisionInput {
    pub(crate) field: String,
    pub(crate) value: Value,
    pub(crate) photos: Vec<DecisionInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DecisionInputPhoto {
    pub(crate) photo_id: String,
    pub(crate) if_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumCreationWire {
    pub(crate) album: AlbumSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumRenameWire {
    pub(crate) album: AlbumSummary,
    pub(crate) renamed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumDeleteWire {
    pub(crate) album_id: String,
    pub(crate) deleted: bool,
    pub(crate) original_files_changed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumAddWire {
    pub(crate) album: AlbumSummary,
    pub(crate) added_photo_ids: Vec<String>,
    pub(crate) already_member_photo_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumRemoveWire {
    pub(crate) album: AlbumSummary,
    pub(crate) removed_photo_ids: Vec<String>,
    pub(crate) already_absent_photo_ids: Vec<String>,
    pub(crate) saved_photo_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlbumReorderWire {
    pub(crate) album: AlbumSummary,
    pub(crate) ordered_photo_ids: Vec<String>,
    pub(crate) reordered: bool,
}

/// One confirmed checked Photo decision batch from the service.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoDecisionWire {
    pub(crate) results: Vec<PhotoDecisionItemWire>,
    pub(crate) counts: PhotoDecisionCountsWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoDecisionCountsWire {
    pub(crate) changed: usize,
    pub(crate) unchanged: usize,
    pub(crate) conflict: usize,
    pub(crate) missing: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoDecisionItemWire {
    pub(crate) photo_id: String,
    pub(crate) outcome: String,
    pub(crate) prior: Option<PhotoDecisionFactsWire>,
    pub(crate) current: Option<PhotoDecisionSnapshotWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoDecisionFactsWire {
    pub(crate) selection_state: SelectionState,
    pub(crate) rating: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoDecisionSnapshotWire {
    pub(crate) selection_state: SelectionState,
    pub(crate) rating: u8,
    pub(crate) decision_version: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRemovalWire {
    pub(crate) operation_id: String,
    pub(crate) counts: PhotoRemovalCountsWire,
    pub(crate) results: Vec<PhotoRemovalItemWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRemovalCountsWire {
    pub(crate) removed: usize,
    pub(crate) changed_elsewhere: usize,
    pub(crate) missing: usize,
    pub(crate) already_removed: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRemovalItemWire {
    pub(crate) photo_id: String,
    pub(crate) outcome: String,
    pub(crate) removed_at_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRestoreWire {
    pub(crate) operation_id: String,
    pub(crate) counts: PhotoRestoreCountsWire,
    pub(crate) results: Vec<PhotoRestoreItemWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRestoreCountsWire {
    pub(crate) restored: usize,
    pub(crate) already_active: usize,
    pub(crate) changed_elsewhere: usize,
    pub(crate) missing: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhotoRestoreItemWire {
    pub(crate) photo_id: String,
    pub(crate) outcome: String,
}

/// The body of one accepted Export submission or replay. The response must
/// echo the revisions this command submitted, because the service captured
/// exactly those revisions for the admitted work.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSubmitWire {
    pub(crate) export_id: String,
    pub(crate) state: String,
    pub(crate) target: String,
    pub(crate) recipe_version: String,
    pub(crate) source_revision: String,
    pub(crate) receipt_expires_at: Option<String>,
    pub(crate) artifact_expires_at: Option<String>,
}

/// One bounded list entry of a Photo's retained Exports.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSummaryWire {
    pub(crate) export_id: String,
    pub(crate) state: String,
    pub(crate) target: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportListWire {
    pub(crate) exports: Vec<ExportSummaryWire>,
}

/// One Export's full inspectable state with the closed artifact object.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportInspectWire {
    pub(crate) export_id: String,
    pub(crate) photo_id: String,
    pub(crate) state: String,
    pub(crate) target: String,
    pub(crate) recipe_version: String,
    pub(crate) source_revision: String,
    pub(crate) bundle_id: String,
    pub(crate) terminal_outcome: Option<String>,
    pub(crate) failure_reason: Option<String>,
    pub(crate) receipt_expires_at: Option<String>,
    pub(crate) artifact: Option<ExportArtifactWire>,
}

/// The closed artifact metadata object. Its fields are exactly the download
/// response headers, so a download is validated field for field against it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportArtifactWire {
    pub(crate) export_id: String,
    pub(crate) target: String,
    pub(crate) stage: String,
    pub(crate) content_type: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) profile_identity: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
    pub(crate) expires_at: String,
}

/// The one Edit Recipe read an Export submission resolves first: the
/// observed source revision and the saved recipe version to submit against.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRecipeSourceWire {
    pub(crate) photo_id: String,
    pub(crate) source_revision: Option<String>,
    pub(crate) recipe: Option<ExportRecipeVersionWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRecipeVersionWire {
    pub(crate) recipe_version: String,
}

pub(crate) fn valid_export_state(state: &str) -> bool {
    matches!(
        state,
        "queued" | "running" | "succeeded" | "failed" | "cancelled"
    )
}

/// The terminal outcome an Export state must report: itself when terminal,
/// otherwise none.
pub(crate) fn export_terminal_outcome(state: &str) -> Option<&'static str> {
    match state {
        "succeeded" => Some("succeeded"),
        "failed" => Some("failed"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Validates one artifact object against the Export it belongs to and the
/// closed per-target stage and media type.
pub(crate) fn validated_export_artifact(
    artifact: &ExportArtifactWire,
    export_id: &str,
    target: &ExportTargetArg,
) -> bool {
    let (stage, content_type) = match target {
        ExportTargetArg::DevelopmentTiff => ("develop", "image/tiff"),
        ExportTargetArg::FilmJpeg => ("film", "image/jpeg"),
    };
    artifact.export_id == export_id
        && ExportTargetArg::parse(&artifact.target).is_some()
        && artifact.stage == stage
        && artifact.content_type == content_type
        && artifact.width > 0
        && artifact.height > 0
        && !artifact.profile_identity.is_empty()
        && artifact.byte_length > 0
        && valid_sha256(&artifact.sha256)
        && valid_utc_time(&artifact.expires_at)
}

/// Validates one inspect response against the requested Export identity and
/// the closed state machine, so the status report only repeats trustworthy
/// facts. An invalid response is a transport failure, not a claimed state.
pub(crate) fn validated_export_inspect(
    data: ExportInspectWire,
    export_id: &str,
    operation: Operation,
) -> Result<ExportInspectWire, CommandFailure> {
    let untrusted = || CommandFailure::transport(operation);
    if data.export_id != export_id
        || data.photo_id.is_empty()
        || !valid_export_state(&data.state)
        || !matches!(
            ExportTargetArg::parse(&data.target),
            Some(ExportTargetArg::DevelopmentTiff | ExportTargetArg::FilmJpeg)
        )
        || data.recipe_version.is_empty()
        || data.source_revision.is_empty()
        || data.bundle_id.is_empty()
        || data.terminal_outcome.as_deref() != export_terminal_outcome(&data.state)
        || data
            .receipt_expires_at
            .as_deref()
            .is_some_and(|time| !valid_utc_time(time))
    {
        return Err(untrusted());
    }
    if let Some(artifact) = &data.artifact {
        let target = ExportTargetArg::parse(&data.target).ok_or_else(untrusted)?;
        if !validated_export_artifact(artifact, export_id, &target) {
            return Err(untrusted());
        }
    }
    Ok(data)
}
