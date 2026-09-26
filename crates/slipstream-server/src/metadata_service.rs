//! Read and checked Save Metadata against the confined Library and the
//! exclusive save supervisor defined by the metadata Design Spec.
use crate::metadata_fields::{self, XmpSource};
use crate::metadata_wire::{
    MetadataAssociation, MetadataAssociationState, MetadataChange, MetadataError,
    MetadataErrorCode, MetadataEvidence, MetadataField, MetadataFieldState, MetadataFileFacts,
    MetadataProvenance, MetadataReadResult, MetadataSaveRequest, MetadataSaveResult,
    MetadataSidecarEvidence, MetadataValue,
};
use serde::Deserialize;
use serde_json::{Value, json};
use slipstream_core::metadata::sidecar::{
    self, ExclusiveSaveLease, ObservedBytes, PublishedSidecar, SidecarEvidence, SidecarFacts,
    SidecarObservation, SupervisorLeaseToken,
};
use slipstream_core::metadata::xmp::XmpDocument;
use slipstream_core::persistence::{MetadataRecord, MetadataStoreError, RetainedOrphan};
use slipstream_core::{
    Library, LibraryRoot, OriginalKind, RelativeOriginalPath, metadata::embedded,
};

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAXIMUM_EMBEDDED_BYTES: u64 = 16 * 1024 * 1024;
const SUPERVISOR_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SUPERVISOR_SAVE_TIMEOUT: Duration = Duration::from_secs(90);
const HELPER_INPUT_LIMIT: usize = 4 * 1024 * 1024;

fn error(code: MetadataErrorCode, message: impl Into<String>) -> MetadataError {
    MetadataError {
        code,
        message: message.into(),
        details: Value::Null,
    }
}

fn map_store(failure: MetadataStoreError) -> MetadataError {
    match failure {
        MetadataStoreError::PhotoMissing => {
            error(MetadataErrorCode::PhotoMissing, "The Photo does not exist.")
        }
        MetadataStoreError::PhotoRemoved => error(
            MetadataErrorCode::PhotoRemoved,
            "The Photo is removed. Restore it before saving metadata.",
        ),
        MetadataStoreError::OriginalUnavailable => error(
            MetadataErrorCode::OriginalUnavailable,
            "The Original File is unavailable.",
        ),
        MetadataStoreError::Storage => error(
            MetadataErrorCode::StorageFailure,
            "The Library state store failed.",
        ),
    }
}

fn join_failure(failure: tokio::task::JoinError) -> MetadataError {
    error(MetadataErrorCode::StorageFailure, failure.to_string())
}

fn file_facts(facts: SidecarFacts) -> MetadataFileFacts {
    MetadataFileFacts {
        device: facts.device,
        inode: facts.inode,
        size: facts.size,
        modified_seconds: facts.modified_seconds,
        modified_nanoseconds: facts.modified_nanoseconds,
    }
}

struct Inspection {
    original_location: String,
    original: RelativeOriginalPath,
    kind: OriginalKind,
    original_facts: SidecarFacts,
    observation: SidecarObservation,
    bytes: ObservedBytes,
    association_generation: u64,
    library_rating: u8,
    removed: bool,
    orphan: Option<RetainedOrphan>,
}

async fn load_record(library: &Library, photo_id: &str) -> Result<MetadataRecord, MetadataError> {
    library
        .with_metadata(photo_id.to_owned(), |context| {
            let record = context.record();
            Ok(MetadataRecord {
                photo_id: record.photo_id.clone(),
                original_id: record.original_id.clone(),
                relative_path: record.relative_path.clone(),
                kind: record.kind,
                association_generation: record.association_generation,
                removed: record.removed,
                library_rating: record.library_rating,
                active: record.active.clone(),
                orphan: record.orphan.clone(),
            })
        })
        .await
        .map_err(map_store)
}

fn inspect_files(record: MetadataRecord, library_root: &Path) -> Result<Inspection, MetadataError> {
    let original = RelativeOriginalPath::parse(record.relative_path.clone()).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The Original Location is invalid.",
        )
    })?;
    let kind = if record.kind == "raw" {
        OriginalKind::Raw
    } else {
        OriginalKind::Jpeg
    };
    let root = LibraryRoot::open(library_root).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The Library Folder cannot be opened.",
        )
    })?;
    let original_facts = sidecar::original_facts(&root, &original).map_err(|_| {
        error(
            MetadataErrorCode::OriginalUnavailable,
            "The Original File cannot be inspected.",
        )
    })?;
    let observation = sidecar::observe(&root, &original, kind).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The Sidecar association cannot be observed.",
        )
    })?;
    let bytes = sidecar::read_observed(&root, &observation).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The Sidecar cannot be read.",
        )
    })?;
    Ok(Inspection {
        original_location: record.relative_path,
        original,
        kind,
        original_facts,
        observation,
        bytes,
        association_generation: record.association_generation,
        library_rating: record.library_rating,
        removed: record.removed,
        orphan: record.orphan,
    })
}

async fn inspect(
    library: &Library,
    library_root: &Path,
    photo_id: &str,
) -> Result<Inspection, MetadataError> {
    let record = load_record(library, photo_id).await?;
    let root = library_root.to_path_buf();
    tokio::task::spawn_blocking(move || inspect_files(record, &root))
        .await
        .map_err(join_failure)?
}

fn sidecar_evidence(
    observation: &SidecarObservation,
) -> (MetadataSidecarEvidence, MetadataAssociation) {
    match observation {
        SidecarObservation::Eligible {
            location,
            facts,
            sha256,
        } => (
            MetadataSidecarEvidence::Present {
                location: location.clone(),
                facts: file_facts(*facts),
                sha256: sha256.clone(),
            },
            MetadataAssociation {
                state: MetadataAssociationState::Eligible,
                candidates: vec![location.clone()],
                reason: None,
            },
        ),
        SidecarObservation::Absent => (
            MetadataSidecarEvidence::Absent,
            MetadataAssociation {
                state: MetadataAssociationState::Eligible,
                candidates: Vec::new(),
                reason: None,
            },
        ),
        SidecarObservation::Ineligible { reason } => (
            MetadataSidecarEvidence::Absent,
            MetadataAssociation {
                state: MetadataAssociationState::Ineligible,
                candidates: Vec::new(),
                reason: Some(reason.clone()),
            },
        ),
        SidecarObservation::Ambiguous { candidates } => (
            MetadataSidecarEvidence::Absent,
            MetadataAssociation {
                state: MetadataAssociationState::Ambiguous,
                candidates: candidates.clone(),
                reason: Some("Multiple candidate Sidecars".into()),
            },
        ),
        SidecarObservation::Invalid { candidate }
        | SidecarObservation::Unreadable { candidate }
        | SidecarObservation::ResourceLimit { candidate } => (
            MetadataSidecarEvidence::Unavailable {
                reason: match observation {
                    SidecarObservation::ResourceLimit { .. } => {
                        "The Sidecar candidate exceeds the 16 MiB read limit."
                    }
                    SidecarObservation::Invalid { .. } => {
                        "The Sidecar candidate is not a readable regular file."
                    }
                    _ => "The Sidecar candidate cannot be read.",
                }
                .into(),
            },
            MetadataAssociation {
                state: MetadataAssociationState::Ineligible,
                candidates: vec![candidate.clone()],
                reason: Some("The Sidecar candidate cannot be used".into()),
            },
        ),
    }
}

fn retained_orphan_blocks(inspection: &Inspection, evidence: &MetadataSidecarEvidence) -> bool {
    let Some(orphan) = &inspection.orphan else {
        return false;
    };
    let MetadataSidecarEvidence::Present {
        location, sha256, ..
    } = evidence
    else {
        return false;
    };
    orphan.sidecar_path == *location && orphan.observed_digest.as_deref() == Some(sha256.as_str())
}

async fn record_observation(
    library: &Library,
    photo_id: &str,
    observation: &SidecarObservation,
) -> Result<(), MetadataError> {
    use slipstream_core::persistence::{ObservedSidecar, ObservedSidecarState};
    let state = match observation {
        SidecarObservation::Eligible {
            location,
            facts,
            sha256,
        } => ObservedSidecarState::Eligible {
            path: location.clone(),
            size: facts.size,
            mtime_ms: facts.modified_seconds as f64 + f64::from(facts.modified_nanoseconds) / 1e9,
            digest: sha256.clone(),
        },
        SidecarObservation::Absent => ObservedSidecarState::Absent,
        SidecarObservation::Invalid { .. }
        | SidecarObservation::Unreadable { .. }
        | SidecarObservation::ResourceLimit { .. } => ObservedSidecarState::Changed,
        SidecarObservation::Ineligible { .. } | SidecarObservation::Ambiguous { .. } => {
            return Ok(());
        }
    };
    library
        .with_metadata(photo_id.to_owned(), move |context| {
            context.record_observation(&ObservedSidecar { state })
        })
        .await
        .map_err(map_store)
}

fn supervisor_exchange(
    socket: &Path,
    payload: &Value,
    timeout: Duration,
) -> Result<Value, MetadataError> {
    let mut stream = UnixStream::connect(socket).map_err(|_| {
        error(
            MetadataErrorCode::SaveUnavailable,
            "The metadata save supervisor is not reachable.",
        )
    })?;
    stream.set_read_timeout(Some(timeout)).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "Supervisor socket configuration failed.",
        )
    })?;
    stream
        .set_write_timeout(Some(SUPERVISOR_CONNECT_TIMEOUT))
        .map_err(|_| {
            error(
                MetadataErrorCode::StorageFailure,
                "Supervisor socket configuration failed.",
            )
        })?;
    let mut body = serde_json::to_vec(payload).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The save request could not be encoded.",
        )
    })?;
    body.push(b'\n');
    stream.write_all(&body).map_err(|_| {
        error(
            MetadataErrorCode::SaveUnavailable,
            "The metadata save supervisor did not accept the request.",
        )
    })?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|_| {
        error(
            MetadataErrorCode::OutcomeUnknown,
            "The metadata save supervisor closed the session without a verdict.",
        )
    })?;
    if response.len() > 4 * 1024 * 1024 {
        return Err(error(
            MetadataErrorCode::StorageFailure,
            "The supervisor response exceeds its limit.",
        ));
    }
    serde_json::from_slice(&response).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The supervisor response is not valid JSON.",
        )
    })
}

async fn supervisor_exchange_offload(
    socket: PathBuf,
    payload: Value,
    timeout: Duration,
) -> Result<Value, MetadataError> {
    tokio::task::spawn_blocking(move || supervisor_exchange(&socket, &payload, timeout))
        .await
        .map_err(join_failure)?
}

fn save_availability(socket: Option<&Path>) -> (bool, Option<String>) {
    let Some(socket) = socket else {
        return (
            false,
            Some(
                "No exclusive metadata save supervisor is configured for this deployment. Read Metadata is supported; Save Metadata is unavailable."
                    .into(),
            ),
        );
    };
    let verdict = supervisor_exchange(
        socket,
        &json!({"operation":"status"}),
        SUPERVISOR_CONNECT_TIMEOUT,
    );
    match verdict {
        Ok(Value::Object(fields)) => {
            if fields.get("available").and_then(Value::as_bool) == Some(true) {
                (true, None)
            } else {
                let reason = fields
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("The supervisor reported that saving is unavailable.")
                    .to_owned();
                (false, Some(reason))
            }
        }
        Ok(_) => (
            false,
            Some("The supervisor status response is malformed.".into()),
        ),
        Err(failure) => (false, Some(failure.message)),
    }
}

fn sidecar_source(observation: &SidecarObservation, bytes: &ObservedBytes) -> XmpSource {
    match bytes {
        ObservedBytes::Bytes(bytes) => metadata_fields::xmp_source(Some(bytes)),
        ObservedBytes::Absent => metadata_fields::xmp_source(None),
        ObservedBytes::Changed => match observation {
            SidecarObservation::ResourceLimit { .. } => XmpSource {
                document: None,
                state: MetadataFieldState::ResourceLimit,
                problem: Some("The Sidecar candidate exceeds the 16 MiB read limit.".into()),
            },
            SidecarObservation::Invalid { .. } | SidecarObservation::Unreadable { .. } => {
                XmpSource {
                    document: None,
                    state: MetadataFieldState::Unavailable,
                    problem: Some("The Sidecar candidate cannot be read.".into()),
                }
            }
            _ => XmpSource {
                document: None,
                state: MetadataFieldState::Invalid,
                problem: Some("The Sidecar changed while it was being read.".into()),
            },
        },
    }
}

pub(crate) async fn read_metadata(
    library: &Library,
    library_root: &Path,
    instance_epoch: &str,
    supervisor: Option<&Path>,
    photo_id: &str,
) -> Result<MetadataReadResult, MetadataError> {
    let inspection = inspect(library, library_root, photo_id).await?;
    let (sidecar, mut association) = sidecar_evidence(&inspection.observation);
    if retained_orphan_blocks(&inspection, &sidecar) {
        association.state = MetadataAssociationState::Unresolved;
        association.reason = Some("retained-orphan".into());
    }
    let root = library_root.to_path_buf();
    let embedded = {
        let inspection_original = inspection.original.clone();
        let inspection_kind = inspection.kind;
        tokio::task::spawn_blocking(move || {
            embedded_metadata(&root, &inspection_original, inspection_kind)
        })
        .await
        .map_err(join_failure)??
    };
    let fields = metadata_fields::read_fields(
        &sidecar_source(&inspection.observation, &inspection.bytes),
        &embedded,
    );
    let capture_facts = metadata_fields::capture_fields(&embedded);
    record_observation(library, photo_id, &inspection.observation).await?;
    let supervisor = supervisor.map(Path::to_path_buf);
    let (save_available, save_unavailable_reason) =
        tokio::task::spawn_blocking(move || save_availability(supervisor.as_deref()))
            .await
            .map_err(join_failure)?;
    let original_location = inspection.original_location.clone();
    Ok(MetadataReadResult {
        photo_id: photo_id.to_owned(),
        original_location: original_location.clone(),
        association,
        fields,
        capture_facts,
        library_rating: inspection.library_rating,
        evidence: MetadataEvidence {
            photo_id: photo_id.to_owned(),
            original_location,
            original: file_facts(inspection.original_facts),
            sidecar,
            association_generation: inspection.association_generation,
            instance_epoch: instance_epoch.to_owned(),
        },
        save_available,
        save_unavailable_reason,
    })
}

fn embedded_metadata(
    library_root: &Path,
    original: &RelativeOriginalPath,
    kind: OriginalKind,
) -> Result<embedded::EmbeddedMetadata, MetadataError> {
    let root = LibraryRoot::open(library_root).map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The Library Folder cannot be opened.",
        )
    })?;
    let capability = root.original(original.clone()).map_err(|_| {
        error(
            MetadataErrorCode::OriginalUnavailable,
            "The Original File cannot be opened.",
        )
    })?;
    match embedded::extract(&capability, kind, MAXIMUM_EMBEDDED_BYTES) {
        Ok(metadata) => Ok(metadata),
        Err(embedded::EmbeddedExtractionError::ResourceLimit) => Ok(embedded::degraded(
            kind,
            embedded::FieldState::ResourceLimit,
        )),
        Err(embedded::EmbeddedExtractionError::InvalidInput) => {
            Ok(embedded::degraded(kind, embedded::FieldState::Invalid))
        }
        Err(embedded::EmbeddedExtractionError::Confinement(_)) => {
            Ok(embedded::degraded(kind, embedded::FieldState::Unavailable))
        }
    }
}

pub(crate) async fn save_metadata(
    library: &Library,
    library_root: &Path,
    instance_epoch: &str,
    supervisor: Option<&Path>,
    photo_id: &str,
    request: MetadataSaveRequest,
) -> Result<MetadataSaveResult, MetadataError> {
    if request.evidence.photo_id != photo_id {
        return Err(error(
            MetadataErrorCode::InvalidInput,
            "The evidence token names another Photo.",
        ));
    }
    let inspection = inspect(library, library_root, photo_id).await?;
    if inspection.removed {
        return Err(error(
            MetadataErrorCode::PhotoRemoved,
            "The Photo is removed. Restore it before saving metadata.",
        ));
    }
    let (sidecar, mut association) = sidecar_evidence(&inspection.observation);
    if retained_orphan_blocks(&inspection, &sidecar) {
        association.state = MetadataAssociationState::Unresolved;
        association.reason = Some("retained-orphan".into());
    }
    let current = MetadataEvidence {
        photo_id: photo_id.to_owned(),
        original_location: inspection.original_location.clone(),
        original: file_facts(inspection.original_facts),
        sidecar: sidecar.clone(),
        association_generation: inspection.association_generation,
        instance_epoch: instance_epoch.to_owned(),
    };
    if request.evidence != current {
        return Err(error(
            MetadataErrorCode::EvidenceStale,
            "The observed metadata changed since the evidence token. Inspect the current metadata and decide again.",
        ));
    }
    if association.state != MetadataAssociationState::Eligible
        || !matches!(
            sidecar,
            MetadataSidecarEvidence::Present { .. } | MetadataSidecarEvidence::Absent
        )
    {
        let reason = association
            .reason
            .unwrap_or_else(|| "The Sidecar association does not permit saving.".into());
        return Err(error(MetadataErrorCode::AssociationUnresolved, reason));
    }
    // Refuse invalid requests before the save session stops external access.
    let mut document = match &inspection.bytes {
        ObservedBytes::Bytes(bytes) => XmpDocument::parse(bytes)
            .map_err(|failure| error(MetadataErrorCode::MetadataMalformed, failure.to_string()))?,
        ObservedBytes::Absent => metadata_fields::empty_document(),
        ObservedBytes::Changed => {
            return Err(error(
                MetadataErrorCode::EvidenceStale,
                "The Sidecar changed while it was being read.",
            ));
        }
    };
    metadata_fields::apply_changes(&mut document, &request.changes)?;
    let Some(supervisor_socket) = supervisor else {
        return Err(error(
            MetadataErrorCode::SaveUnavailable,
            "No exclusive metadata save supervisor is configured for this deployment. Read Metadata is supported; Save Metadata is unavailable.",
        ));
    };
    let status = supervisor_exchange_offload(
        supervisor_socket.to_path_buf(),
        json!({"operation":"status"}),
        SUPERVISOR_CONNECT_TIMEOUT,
    )
    .await?;
    if status.get("available").and_then(Value::as_bool) != Some(true) {
        let reason = status
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("The supervisor reported that saving is unavailable.");
        return Err(error(MetadataErrorCode::SaveUnavailable, reason));
    }
    let helper_sidecar = match &sidecar {
        MetadataSidecarEvidence::Present {
            location,
            facts,
            sha256,
        } => json!({
            "state": "present",
            "location": location,
            "facts": facts,
            "sha256": sha256,
        }),
        MetadataSidecarEvidence::Absent => json!({"state": "absent"}),
        MetadataSidecarEvidence::Unavailable { .. } => {
            unreachable!("unavailable sidecar evidence refuses the save earlier")
        }
    };
    let payload = json!({
        "operation": "save",
        "request": {
            "originalPath": inspection.original_location,
            "originalFacts": inspection.original_facts,
            "sidecarEvidence": helper_sidecar,
            "changes": request.changes,
        }
    });
    let verdict = supervisor_exchange_offload(
        supervisor_socket.to_path_buf(),
        payload,
        SUPERVISOR_SAVE_TIMEOUT,
    )
    .await?;
    let outcome = parse_supervisor_verdict(verdict)?;
    let location = outcome
        .get("location")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            error(
                MetadataErrorCode::StorageFailure,
                "The save session omitted the Sidecar location.",
            )
        })?
        .to_owned();
    let facts: SidecarFacts = serde_json::from_value(
        outcome.get("facts").cloned().unwrap_or(Value::Null),
    )
    .map_err(|_| {
        error(
            MetadataErrorCode::StorageFailure,
            "The save session returned invalid Sidecar facts.",
        )
    })?;
    let sha256 = outcome
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            error(
                MetadataErrorCode::StorageFailure,
                "The save session omitted the Sidecar digest.",
            )
        })?
        .to_owned();
    record_observation(
        library,
        photo_id,
        &SidecarObservation::Eligible {
            location: location.clone(),
            facts,
            sha256: sha256.clone(),
        },
    )
    .await?;
    let mut verified_values = BTreeMap::new();
    if let Some(verified) = outcome.get("verified").and_then(Value::as_object) {
        for (name, entry) in verified {
            let state = serde_json::from_value::<MetadataFieldState>(
                entry.get("state").cloned().unwrap_or(Value::Null),
            )
            .unwrap_or(MetadataFieldState::Invalid);
            let value = entry
                .get("value")
                .and_then(|value| serde_json::from_value::<MetadataValue>(value.clone()).ok());
            verified_values.insert(
                name.clone(),
                MetadataField {
                    state,
                    writable: true,
                    provenance: Some(MetadataProvenance::Sidecar),
                    value,
                    problem: None,
                    inferred_value: None,
                    sources: Vec::new(),
                },
            );
        }
    }
    let affected_fields: Vec<String> = request.changes.keys().cloned().collect();
    Ok(MetadataSaveResult {
        photo_id: photo_id.to_owned(),
        sidecar_location: location.clone(),
        affected_fields,
        verified_values,
        evidence: MetadataEvidence {
            photo_id: photo_id.to_owned(),
            original_location: inspection.original_location,
            original: file_facts(inspection.original_facts),
            sidecar: MetadataSidecarEvidence::Present {
                location,
                facts: file_facts(facts),
                sha256,
            },
            association_generation: inspection.association_generation,
            instance_epoch: instance_epoch.to_owned(),
        },
    })
}

fn parse_supervisor_verdict(verdict: Value) -> Result<Value, MetadataError> {
    if let Some(failure) = verdict.get("error") {
        let code = failure
            .get("code")
            .and_then(Value::as_str)
            .and_then(|code| {
                serde_json::from_value::<MetadataErrorCode>(Value::String(code.to_owned())).ok()
            })
            .unwrap_or(MetadataErrorCode::StorageFailure);
        let message = failure
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The save session failed.")
            .to_owned();
        let details = failure.get("details").cloned().unwrap_or(Value::Null);
        return Err(MetadataError {
            code,
            message,
            details,
        });
    }
    verdict.get("ok").cloned().ok_or_else(|| {
        error(
            MetadataErrorCode::StorageFailure,
            "The save session returned no verdict.",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperRequest {
    original_path: String,
    original_facts: SidecarFacts,
    sidecar_evidence: HelperSidecarEvidence,
    changes: BTreeMap<String, MetadataChange>,
}

#[derive(Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum HelperSidecarEvidence {
    Present {
        location: String,
        facts: SidecarFacts,
        sha256: String,
    },
    Absent,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperEnvelope {
    lease_token: String,
    save: HelperRequest,
}

/// Entry point of the `metadata-save-helper` binary the supervisor launches
/// inside the exclusive save session. It rechecks the evidence, validates the
/// patches against the current document, publishes, and verifies.
pub fn helper_entrypoint() -> i32 {
    let mut arguments = std::env::args().skip(1);
    let mut root = None;
    let mut lease_fd = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = arguments.next(),
            "--lease-fd" => lease_fd = arguments.next().and_then(|value| value.parse::<i32>().ok()),
            _ => {}
        }
    }
    let response = match run_helper(root.as_deref(), lease_fd) {
        Ok(response) => response,
        Err(message) => {
            json!({"error": {"code": "invalid_input", "message": message, "details": null}})
        }
    };
    let mut stdout = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, &response);
    let _ = writeln!(stdout);
    0
}

fn run_helper(root: Option<&str>, lease_fd: Option<i32>) -> Result<Value, String> {
    let root = root.ok_or_else(|| "The helper requires --root.".to_owned())?;
    let lease_fd = lease_fd.ok_or_else(|| "The helper requires --lease-fd.".to_owned())?;
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(HELPER_INPUT_LIMIT as u64)
        .read_to_end(&mut input)
        .map_err(|_| "The helper could not read its request.".to_owned())?;
    let envelope: HelperEnvelope =
        serde_json::from_slice(&input).map_err(|_| "The helper request is invalid.".to_owned())?;
    let request = envelope.save;
    let failure = |code: &str, message: String| json!({"error": {"code": code, "message": message, "details": null}});
    let root =
        LibraryRoot::open(root).map_err(|_| "The backing root cannot be opened.".to_owned())?;
    let original = RelativeOriginalPath::parse(request.original_path.clone())
        .map_err(|_| "The Original Location is invalid.".to_owned())?;
    let observed_original = sidecar::original_facts(&root, &original)
        .map_err(|_| "The Original File cannot be inspected.".to_owned())?;
    if observed_original != request.original_facts {
        return Ok(failure(
            "evidence_stale",
            "The Original File changed since the evidence token.".into(),
        ));
    }
    let (evidence, observation, location) = match &request.sidecar_evidence {
        HelperSidecarEvidence::Present {
            location,
            facts,
            sha256,
        } => (
            SidecarEvidence::Present {
                facts: *facts,
                sha256: sha256.clone(),
            },
            SidecarObservation::Eligible {
                location: location.clone(),
                facts: *facts,
                sha256: sha256.clone(),
            },
            location.clone(),
        ),
        HelperSidecarEvidence::Absent => {
            let filename = original.as_str().rsplit('/').next().unwrap_or_default();
            let stem = slipstream_core::identity::pairing_stem(filename);
            let location = original.as_str().rsplit_once('/').map_or_else(
                || format!("{stem}.xmp"),
                |(parent, _)| format!("{parent}/{stem}.xmp"),
            );
            (
                SidecarEvidence::Absent,
                SidecarObservation::Absent,
                location,
            )
        }
    };
    let bytes = sidecar::read_observed(&root, &observation)
        .map_err(|_| "The Sidecar cannot be read.".to_owned())?;
    let mut document = match &bytes {
        ObservedBytes::Bytes(bytes) => XmpDocument::parse(bytes)
            .map_err(|refusal| format!("The Sidecar document is malformed: {refusal}"))?,
        ObservedBytes::Absent => metadata_fields::empty_document(),
        ObservedBytes::Changed => {
            return Ok(failure(
                "evidence_stale",
                "The Sidecar changed since the evidence token.".into(),
            ));
        }
    };
    // The supervisor lease descriptor stays open for this process's whole
    // lifetime, keeping the save session bound to the helper.
    let _ = lease_fd;
    if let Err(refusal) = metadata_fields::apply_changes(&mut document, &request.changes) {
        return Ok(json!({
            "error": {
                "code": refusal.code,
                "message": refusal.message,
                "details": refusal.details,
            }
        }));
    }
    let serialized = document
        .to_bytes()
        .map_err(|refusal| format!("The Sidecar document cannot be serialized: {refusal}"))?;
    let lease =
        ExclusiveSaveLease::from_supervisor_token(SupervisorLeaseToken(envelope.lease_token));
    let published: PublishedSidecar =
        match sidecar::publish(&root, &original, &evidence, &lease, &serialized) {
            Ok(published) => published,
            Err(sidecar::PublishError::Conflict) => {
                return Ok(failure(
                    "evidence_stale",
                    "The Sidecar conflicts with the observed evidence.".into(),
                ));
            }
            Err(sidecar::PublishError::StorageUnavailable) => {
                return Ok(failure(
                    "storage_failure",
                    "The Sidecar storage is unavailable.".into(),
                ));
            }
            Err(sidecar::PublishError::ResourceLimit(reason)) => {
                return Ok(failure("resource_limit", reason));
            }
            Err(sidecar::PublishError::OutcomeUnknown) => {
                return Ok(failure(
                    "outcome_unknown",
                    "The Sidecar publication outcome is unknown.".into(),
                ));
            }
        };
    let mut verified = serde_json::Map::new();
    for name in request.changes.keys() {
        let (state, value) = metadata_fields::xmp_value(&document, name);
        verified.insert(name.clone(), json!({"state": state, "value": value}));
    }
    Ok(json!({
        "ok": {
            "location": location,
            "facts": published.facts,
            "sha256": published.sha256,
            "verified": verified,
        }
    }))
}
