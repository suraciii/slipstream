//! The Export lifecycle: submission against the recipe and source revisions
//! this command observed, retained-Export inspection and listing, exactly-
//! once cancellation, and retained-snapshot retry. Every write is one
//! request; a response that cannot prove its own outcome stays an unknown
//! outcome instead of a claimed receipt or refusal, no write is retried
//! automatically, and a retry never reads the current recipe to refresh the
//! captured snapshot's guards.

use super::{
    AdmissionState, CommandFailure, ErrorPayload, ExportTargetArg, MutationIdentity, Operation,
    PhotoExportRetryArgs, PhotoExportSubmitArgs, ServiceClient, valid_request_identity,
    valid_sha256, valid_utc_time,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------- reads

/// Lists one Photo's retained Exports in retention order.
pub(super) async fn list(
    client: &ServiceClient,
    photo_id: &str,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let data: ExportListWire = client
        .json(
            operation,
            Method::GET,
            client.endpoint(&["api", "photos", photo_id, "exports"]),
            None,
        )
        .await?;
    export_list_value(data, operation)
}

/// Inspects one Export's current state and retained artifact facts.
pub(super) async fn status(
    client: &ServiceClient,
    export_id: &str,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let inspected = inspect(client, export_id, operation).await?;
    serde_json::to_value(inspected).map_err(|_| CommandFailure::transport(operation))
}

/// Reads and validates one Export inspection against the requested identity.
async fn inspect(
    client: &ServiceClient,
    export_id: &str,
    operation: Operation,
) -> Result<ExportInspectWire, CommandFailure> {
    let data: ExportInspectWire = client
        .json(
            operation,
            Method::GET,
            client.endpoint(&["api", "exports", export_id]),
            None,
        )
        .await?;
    validated_export_inspect(data, export_id, operation)
}

// ---------------------------------------------------------------- submit

/// Resolves the current Edit Recipe and source revision, then submits the
/// Export against exactly those observed revisions, so the service captures
/// what this command saw instead of whatever is current at admission. The
/// submission is a write: any unusable response stays an unknown outcome.
pub(super) async fn submit(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &PhotoExportSubmitArgs,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let read: ExportRecipeSourceWire = client
        .json(
            operation,
            Method::GET,
            client.endpoint(&["api", "photos", &args.photo_id, "edit-recipe"]),
            None,
        )
        .await?;
    if read.photo_id != args.photo_id {
        return Err(CommandFailure::transport(operation));
    }
    let Some(recipe) = read.recipe else {
        return Err(export_refusal(
            3,
            "missing_recipe",
            "Save an Edit Recipe for this Photo before submitting an Export.",
        ));
    };
    if recipe.recipe_version.is_empty() {
        return Err(CommandFailure::transport(operation));
    }
    let Some(source_revision) = read.source_revision.filter(|revision| !revision.is_empty()) else {
        return Err(export_refusal(
            6,
            "resource_unavailable",
            "The current source revision cannot be read; retry when the Library reports the source as available.",
        ));
    };
    let identity = MutationIdentity {
        operation,
        photo_ids: vec![args.photo_id.clone()],
        album_id: None,
        album_name: None,
    };
    let result: ExportSubmitWire = client
        .mutation_admitting(
            &identity,
            admission,
            client.endpoint(&["api", "photos", &args.photo_id, "exports"]),
            Some(json!({
                "requestId": args.request_id,
                "expectedRecipeVersion": recipe.recipe_version,
                "expectedSourceRevision": source_revision,
                "target": args.target.wire(),
            })),
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await?;
    confirmed_export_submit(
        &identity,
        args.target.wire(),
        &recipe.recipe_version,
        &source_revision,
        result,
    )
}

// ---------------------------------------------------------------- cancel

/// Cancels one Export. The Export is read first so an uncertain cancellation
/// can name its Photo; that read is not a guard, because the cancellation
/// settles exactly once against the actual completion state. A completion
/// that raced the request is therefore reported as the terminal settlement
/// it produced, including success.
pub(super) async fn cancel(
    client: &ServiceClient,
    admission: &AdmissionState,
    export_id: &str,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let identity = export_photo_identity(client, export_id, operation).await?;
    let result: ExportCancelWire = client
        .mutation_admitting(
            &identity,
            admission,
            client.endpoint(&["api", "exports", export_id, "cancel"]),
            // The route carries no request fields; the settlement is owned
            // by the Export identity alone.
            None,
            &[StatusCode::OK],
        )
        .await?;
    confirmed_export_cancel(&identity, export_id, result)
}

// ---------------------------------------------------------------- retry

/// Retries one failed or cancelled Export against its retained snapshot with
/// the caller's new request identity. The snapshot owns the captured recipe,
/// source, and bundle, so this command never reads the current recipe: the
/// request identity is the only input. A replay of an identity the service
/// already accepted resolves to the same Export and starts no new attempt.
pub(super) async fn retry(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &PhotoExportRetryArgs,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let identity = export_photo_identity(client, &args.export_id, operation).await?;
    let result: ExportRetryWire = client
        .mutation_admitting(
            &identity,
            admission,
            client.endpoint(&["api", "exports", &args.export_id, "retry"]),
            Some(json!({ "requestId": args.request_id })),
            &[StatusCode::ACCEPTED],
        )
        .await?;
    confirmed_export_retry(&identity, &args.export_id, result)
}

/// Reads one Export to identify the Photo behind an export-scoped write, so
/// an uncertain outcome can name the affected Photo. The read is not a
/// guard: the cancellation owns its settlement and the retry uses the
/// retained snapshot. A confirmed refusal, such as an unknown Export, is the
/// write's own refusal and stops before any request is sent.
async fn export_photo_identity(
    client: &ServiceClient,
    export_id: &str,
    operation: Operation,
) -> Result<MutationIdentity, CommandFailure> {
    let inspected = inspect(client, export_id, operation).await?;
    Ok(MutationIdentity {
        operation,
        photo_ids: vec![inspected.photo_id],
        album_id: None,
        album_name: None,
    })
}

// ---------------------------------------------------------------- receipts

/// One confirmed development-surface refusal synthesized locally under the
/// same closed code the service uses for the same outcome.
fn export_refusal(exit_code: u8, code: &'static str, message: &'static str) -> CommandFailure {
    CommandFailure::from_payload(
        exit_code,
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: "none".to_owned(),
            details: json!({}),
        },
    )
}

/// Validates one confirmed submit response against the submitted request.
/// The response must repeat the requested target and the exact revisions
/// this command submitted; anything else is an unknown outcome rather than
/// a claimed receipt.
fn confirmed_export_submit(
    identity: &MutationIdentity,
    target: &str,
    recipe_version: &str,
    source_revision: &str,
    result: ExportSubmitWire,
) -> Result<Value, CommandFailure> {
    let unknown = || CommandFailure::unknown(identity);
    if !valid_request_identity(&result.export_id)
        || !valid_export_state(&result.state)
        || result.target != target
        || result.recipe_version != recipe_version
        || result.source_revision != source_revision
        || result
            .receipt_expires_at
            .as_deref()
            .is_some_and(|time| !valid_utc_time(time))
        || result
            .artifact_expires_at
            .as_deref()
            .is_some_and(|time| !valid_utc_time(time))
    {
        return Err(unknown());
    }
    serde_json::to_value(result).map_err(|_| unknown())
}

/// Validates one confirmed cancellation against the requested Export. The
/// response must name that Export and carry a terminal settlement consistent
/// with its state; a response that settles nothing is not a cancellation
/// receipt.
fn confirmed_export_cancel(
    identity: &MutationIdentity,
    export_id: &str,
    result: ExportCancelWire,
) -> Result<Value, CommandFailure> {
    if result.export_id != export_id
        || export_terminal_outcome(&result.state)
            .is_none_or(|outcome| result.terminal_outcome.as_deref() != Some(outcome))
    {
        return Err(CommandFailure::unknown(identity));
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::unknown(identity))
}

/// Validates one confirmed retry against the requested Export. The response
/// must name that Export and carry a closed state; the state is the
/// service's current truth, including a replayed identity whose attempt has
/// already settled again.
fn confirmed_export_retry(
    identity: &MutationIdentity,
    export_id: &str,
    result: ExportRetryWire,
) -> Result<Value, CommandFailure> {
    if result.export_id != export_id || !valid_export_state(&result.state) {
        return Err(CommandFailure::unknown(identity));
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::unknown(identity))
}

/// Renders one validated retained-Export list. Every entry must carry a
/// closed state and target; an invalid entry is a transport failure.
fn export_list_value(data: ExportListWire, operation: Operation) -> Result<Value, CommandFailure> {
    for export in &data.exports {
        if !valid_request_identity(&export.export_id)
            || !valid_export_state(&export.state)
            || ExportTargetArg::parse(&export.target).is_none()
        {
            return Err(CommandFailure::transport(operation));
        }
    }
    serde_json::to_value(data).map_err(|_| CommandFailure::transport(operation))
}

// ---------------------------------------------------------------- shapes

/// The body of one accepted Export submission or replay. The response must
/// echo the revisions this command submitted, because the service captured
/// exactly those revisions for the admitted work.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportSubmitWire {
    export_id: String,
    state: String,
    target: String,
    recipe_version: String,
    source_revision: String,
    receipt_expires_at: Option<String>,
    artifact_expires_at: Option<String>,
}

/// One bounded list entry of a Photo's retained Exports.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportSummaryWire {
    export_id: String,
    state: String,
    target: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportListWire {
    exports: Vec<ExportSummaryWire>,
}

/// One Export's full inspectable state with the closed artifact object.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportInspectWire {
    pub(super) export_id: String,
    pub(super) photo_id: String,
    pub(super) state: String,
    pub(super) target: String,
    pub(super) recipe_version: String,
    pub(super) source_revision: String,
    pub(super) bundle_id: String,
    pub(super) terminal_outcome: Option<String>,
    pub(super) failure_reason: Option<String>,
    pub(super) receipt_expires_at: Option<String>,
    pub(super) artifact: Option<ExportArtifactWire>,
}

/// The closed artifact metadata object. Its fields are exactly the download
/// response headers, so a download is validated field for field against it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportArtifactWire {
    pub(super) export_id: String,
    pub(super) target: String,
    pub(super) stage: String,
    pub(super) content_type: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) profile_identity: String,
    pub(super) byte_length: u64,
    pub(super) sha256: String,
    pub(super) expires_at: String,
}

/// The settled Export one cancellation returns: the Export identity, its
/// terminal state, and the matching terminal outcome.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportCancelWire {
    export_id: String,
    state: String,
    terminal_outcome: Option<String>,
}

/// The admitted or replayed retry attempt: the Export identity and its
/// current state.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportRetryWire {
    export_id: String,
    state: String,
}

/// The one Edit Recipe read an Export submission resolves first: the
/// observed source revision and the saved recipe version to submit against.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRecipeSourceWire {
    photo_id: String,
    source_revision: Option<String>,
    recipe: Option<ExportRecipeVersionWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRecipeVersionWire {
    recipe_version: String,
}

fn valid_export_state(state: &str) -> bool {
    matches!(
        state,
        "queued" | "running" | "succeeded" | "failed" | "cancelled"
    )
}

/// The terminal outcome an Export state must report: itself when terminal,
/// otherwise none.
fn export_terminal_outcome(state: &str) -> Option<&'static str> {
    match state {
        "succeeded" => Some("succeeded"),
        "failed" => Some("failed"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}

/// Validates one artifact object against the Export it belongs to and the
/// closed per-target stage and media type.
fn validated_export_artifact(
    artifact: &ExportArtifactWire,
    export_id: &str,
    target: &ExportTargetArg,
) -> bool {
    let (stage, content_type) = match target {
        ExportTargetArg::DevelopmentTiff => ("develop", "image/tiff"),
        ExportTargetArg::FilmJpeg => ("film", "image/jpeg"),
    };
    artifact.export_id == export_id
        && artifact.target == target.wire()
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
pub(super) fn validated_export_inspect(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_submit_response_must_echo_the_captured_revisions() {
        let identity = MutationIdentity {
            operation: Operation::PhotosExportSubmit,
            photo_ids: vec!["p1".to_owned()],
            album_id: None,
            album_name: None,
        };
        let confirmed = |result: ExportSubmitWire| {
            confirmed_export_submit(&identity, "film-jpeg", "recipe-2", "source-7", result)
        };
        let submit = |target: &str, recipe: &str, source: &str| ExportSubmitWire {
            export_id: "e1".to_owned(),
            state: "queued".to_owned(),
            target: target.to_owned(),
            recipe_version: recipe.to_owned(),
            source_revision: source.to_owned(),
            receipt_expires_at: None,
            artifact_expires_at: None,
        };
        let confirmed_value =
            confirmed(submit("film-jpeg", "recipe-2", "source-7")).expect("echoes submission");
        assert_eq!(confirmed_value["exportId"], "e1");
        assert_eq!(confirmed_value["target"], "film-jpeg");
        assert_eq!(confirmed_value["receiptExpiresAt"], Value::Null);
        for label in [
            ("target", submit("development-tiff", "recipe-2", "source-7")),
            ("recipe", submit("film-jpeg", "recipe-1", "source-7")),
            ("source", submit("film-jpeg", "recipe-2", "source-8")),
        ] {
            let failure = confirmed(label.1).unwrap_err();
            assert_eq!(failure.exit_code, 7, "for {}", label.0);
            assert_eq!(failure.payload.code, "outcome_unknown", "for {}", label.0);
        }
        let unknown_identity = submit("film-jpeg", "recipe-2", "source-7");
        let unknown = confirmed(ExportSubmitWire {
            export_id: "space id".to_owned(),
            ..unknown_identity
        })
        .unwrap_err();
        assert_eq!(unknown.exit_code, 7);
        let stale_time = confirmed(ExportSubmitWire {
            receipt_expires_at: Some("yesterday".to_owned()),
            ..submit("film-jpeg", "recipe-2", "source-7")
        })
        .unwrap_err();
        assert_eq!(stale_time.exit_code, 7);
    }

    #[test]
    fn export_cancellation_receipts_must_settle_a_terminal_state() {
        let identity = MutationIdentity {
            operation: Operation::PhotosExportCancel,
            photo_ids: vec!["p1".to_owned()],
            album_id: None,
            album_name: None,
        };
        let cancel = |state: &str, terminal: Option<&str>| {
            confirmed_export_cancel(
                &identity,
                "e1",
                ExportCancelWire {
                    export_id: "e1".to_owned(),
                    state: state.to_owned(),
                    terminal_outcome: terminal.map(str::to_owned),
                },
            )
        };
        // A completion that raced the cancellation is the settlement the
        // response must report, including success.
        let settled = cancel("succeeded", Some("succeeded")).expect("raced completion is settled");
        assert_eq!(settled["terminalOutcome"], "succeeded");
        cancel("cancelled", Some("cancelled")).expect("cancellation is settled");
        for label in [
            ("unsettled state", cancel("queued", None)),
            ("unsettled outcome", cancel("cancelled", None)),
            ("wrong outcome", cancel("failed", Some("succeeded"))),
            ("outcome on active", cancel("running", Some("cancelled"))),
            ("closed set violation", cancel("expired", Some("expired"))),
        ] {
            let failure = label.1.unwrap_err();
            assert_eq!(failure.exit_code, 7, "for {}", label.0);
            assert_eq!(failure.payload.code, "outcome_unknown", "for {}", label.0);
        }
        let wrong_export = confirmed_export_cancel(
            &identity,
            "e1",
            ExportCancelWire {
                export_id: "other".to_owned(),
                state: "cancelled".to_owned(),
                terminal_outcome: Some("cancelled".to_owned()),
            },
        )
        .unwrap_err();
        assert_eq!(wrong_export.exit_code, 7);
    }

    #[test]
    fn export_retry_receipts_must_name_the_retried_export() {
        let identity = MutationIdentity {
            operation: Operation::PhotosExportRetry,
            photo_ids: vec!["p1".to_owned()],
            album_id: None,
            album_name: None,
        };
        let retry = |export_id: &str, state: &str| {
            confirmed_export_retry(
                &identity,
                "e1",
                ExportRetryWire {
                    export_id: export_id.to_owned(),
                    state: state.to_owned(),
                },
            )
        };
        assert_eq!(retry("e1", "queued").unwrap()["state"], "queued");
        // A replayed identity whose attempt settled again reports the
        // service's current state; the receipt stays valid.
        retry("e1", "failed").expect("a replayed attempt may have settled again");
        for label in [
            ("wrong export", retry("other", "queued")),
            ("closed set violation", retry("e1", "expired")),
        ] {
            let failure = label.1.unwrap_err();
            assert_eq!(failure.exit_code, 7, "for {}", label.0);
            assert_eq!(failure.payload.code, "outcome_unknown", "for {}", label.0);
        }
    }

    #[test]
    fn export_inspection_is_validated_against_the_closed_state_machine() {
        let artifact = ExportArtifactWire {
            export_id: "e1".to_owned(),
            target: "development-tiff".to_owned(),
            stage: "develop".to_owned(),
            content_type: "image/tiff".to_owned(),
            width: 5542,
            height: 3696,
            profile_identity: "profile".to_owned(),
            byte_length: 4096,
            sha256: "a".repeat(64),
            expires_at: "2026-01-01T12:00:00Z".to_owned(),
        };
        let inspect =
            |state: &str, terminal: Option<&str>, artifact: Option<ExportArtifactWire>| {
                ExportInspectWire {
                    export_id: "e1".to_owned(),
                    photo_id: "p1".to_owned(),
                    state: state.to_owned(),
                    target: "development-tiff".to_owned(),
                    recipe_version: "recipe-2".to_owned(),
                    source_revision: "source-7".to_owned(),
                    bundle_id: "bundle".to_owned(),
                    terminal_outcome: terminal.map(str::to_owned),
                    failure_reason: None,
                    receipt_expires_at: None,
                    artifact,
                }
            };
        let value = validated_export_inspect(
            inspect("running", None, None),
            "e1",
            Operation::PhotosExportStatus,
        )
        .expect("running inspection is valid");
        assert_eq!(value.state, "running");
        validated_export_inspect(
            inspect("succeeded", Some("succeeded"), Some(artifact.clone())),
            "e1",
            Operation::PhotosExportStatus,
        )
        .expect("succeeded inspection is valid");

        let transport = |data: ExportInspectWire| {
            validated_export_inspect(data, "e1", Operation::PhotosExportStatus).unwrap_err()
        };
        for label in [
            ("unknown state", inspect("expired", None, None)),
            ("wrong terminal", inspect("failed", Some("succeeded"), None)),
            (
                "terminal on active",
                inspect("queued", Some("queued"), None),
            ),
            ("unknown target", {
                let mut data = inspect("succeeded", Some("succeeded"), None);
                data.target = "gallery-print".to_owned();
                data
            }),
        ] {
            let failure = transport(label.1);
            assert_eq!(failure.payload.code, "transport_failed", "for {}", label.0);
        }
        // The artifact object must name its own Export with the closed
        // per-target stage and media type.
        for label in [
            ("wrong export", {
                let mut artifact = artifact.clone();
                artifact.export_id = "other".to_owned();
                inspect("succeeded", Some("succeeded"), Some(artifact))
            }),
            ("artifact target differs from export", {
                let mut artifact = artifact.clone();
                artifact.target = "film-jpeg".to_owned();
                inspect("succeeded", Some("succeeded"), Some(artifact))
            }),
            ("jpeg stage on tiff target", {
                let mut artifact = artifact.clone();
                artifact.stage = "film".to_owned();
                inspect("succeeded", Some("succeeded"), Some(artifact))
            }),
            ("wrong media type", {
                let mut artifact = artifact.clone();
                artifact.content_type = "image/jpeg".to_owned();
                inspect("succeeded", Some("succeeded"), Some(artifact))
            }),
            ("short digest", {
                let mut artifact = artifact;
                artifact.sha256 = "a".repeat(63);
                inspect("succeeded", Some("succeeded"), Some(artifact))
            }),
        ] {
            let failure = transport(label.1);
            assert_eq!(failure.payload.code, "transport_failed", "for {}", label.0);
        }
    }

    #[test]
    fn export_list_entries_carry_only_closed_states_and_targets() {
        let entry = |target: &str| ExportSummaryWire {
            export_id: "e1".to_owned(),
            state: "succeeded".to_owned(),
            target: target.to_owned(),
        };
        assert_eq!(
            export_list_value(
                ExportListWire {
                    exports: vec![entry("film-jpeg")]
                },
                Operation::PhotosExportList
            )
            .unwrap()["exports"][0]["target"],
            "film-jpeg"
        );
        assert_eq!(
            export_list_value(
                ExportListWire {
                    exports: vec![entry("development-tiff")]
                },
                Operation::PhotosExportList
            )
            .unwrap()["exports"][0]["target"],
            "development-tiff"
        );
        assert_eq!(
            export_list_value(
                ExportListWire {
                    exports: Vec::new()
                },
                Operation::PhotosExportList
            )
            .unwrap()["exports"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        for target in ["gallery-print", ""] {
            let failure = export_list_value(
                ExportListWire {
                    exports: vec![entry(target)],
                },
                Operation::PhotosExportList,
            )
            .unwrap_err();
            assert_eq!(failure.payload.code, "transport_failed", "for {target}");
        }
    }
}
