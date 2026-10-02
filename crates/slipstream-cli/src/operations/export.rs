use super::*;

/// One confirmed development-surface refusal synthesized locally under the
/// same closed code the service uses for the same outcome.
pub(crate) fn export_refusal(
    exit_code: u8,
    code: &'static str,
    message: &'static str,
) -> CommandFailure {
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

/// Resolves the current Edit Recipe and source revision, then submits the
/// Export against exactly those observed revisions, so the service captures
/// what this command saw instead of whatever is current at admission. The
/// submission is a write: any unusable response stays an unknown outcome.
pub(crate) async fn export_submission(
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
        mappings: Vec::new(),
    };
    let result: ExportSubmitWire = client
        .mutation_admitting(
            &identity,
            admission,
            client.endpoint(&["api", "photos", &args.photo_id, "exports"]),
            json!({
                "requestId": args.request_id,
                "expectedRecipeVersion": recipe.recipe_version,
                "expectedSourceRevision": source_revision,
                "target": args.target.wire(),
            }),
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

/// Validates one confirmed submit response against the submitted request.
/// The response must repeat the requested target and the exact revisions
/// this command submitted; anything else is an unknown outcome rather than
/// a claimed receipt.
pub(crate) fn confirmed_export_submit(
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

/// Renders one validated retained-Export list. Every entry must carry a
/// closed state and target; an invalid entry is a transport failure.
pub(crate) fn export_list_value(
    data: ExportListWire,
    operation: Operation,
) -> Result<Value, CommandFailure> {
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
