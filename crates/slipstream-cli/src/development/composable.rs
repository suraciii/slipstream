use super::*;
// ------------------------------------------------ composable processing steps
/// The published identity bounds the composable Processing Step wire
/// contract shares with the service's own admission. They are closed and
/// module-independent, so a conforming client can always construct a
/// wire-valid recipe before any module admission check.
pub(super) const MAXIMUM_STEP_ID_BYTES: usize = 64;
pub(super) const MAXIMUM_MODULE_ID_BYTES: usize = 64;
pub(super) const MAXIMUM_ARTIFACT_ID_BYTES: usize = 128;
pub(super) const MAXIMUM_PHOTO_ID_BYTES: usize = 128;
pub(super) const MAXIMUM_REVISION_BYTES: usize = 128;
pub(super) const MAXIMUM_CONTRACT_NAME_BYTES: usize = 128;
pub(super) const MAXIMUM_RECIPE_STEPS: usize = 64;
pub(super) const MAXIMUM_GEOMETRY_EDGE: u32 = 65_536;
pub(super) const MAXIMUM_PARAMETER_TREE_DEPTH: usize = 32;

/// The `photos processing-recipe` subcommands. The complete intended recipe
/// travels in one `--input` document: the caller composes zero or more
/// module-owned steps and selects the current one, and this module never
/// completes a guard, reorders steps, or interprets a parameter tree behind
/// the caller's back.
#[derive(Debug, clap::Subcommand)]
pub enum ProcessingRecipeCommand {
    /// Read one Photo's composable Processing Recipe and observed source
    /// revision.
    Get {
        /// One Photo ID.
        #[arg(value_name = "PHOTO_ID", value_parser = crate::nonempty)]
        photo_id: String,
    },
    /// Guarded save of the complete composable recipe with explicit
    /// revisions.
    Save(ProcessingRecipeWriteArgs),
    /// Rebind retained intent to the newly observed Original revision.
    Rebind(ProcessingRecipeWriteArgs),
}

#[derive(Debug, clap::Args)]
pub struct ProcessingRecipeWriteArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = crate::nonempty)]
    pub photo_id: String,
    /// UTF-8 JSON file holding the complete guarded recipe body; `-` reads
    /// standard input.
    #[arg(long, value_name = "FILE", value_parser = crate::nonempty)]
    pub input: String,
}

/// Reads and validates the composable save body before any network access.
/// A `Get` needs no input; the save reads its complete document once and
/// returns the exact serialized body the mutation later submits.
pub(crate) async fn prepare_processing(
    command: &ProcessingRecipeCommand,
) -> Result<Option<Value>, CommandFailure> {
    match command {
        ProcessingRecipeCommand::Get { .. } => Ok(None),
        ProcessingRecipeCommand::Save(args) => Ok(Some(parse_processing_save(
            read_input_bytes(&args.input).await?,
        )?)),
        ProcessingRecipeCommand::Rebind(args) => Ok(Some(parse_processing_rebind(
            read_input_bytes(&args.input).await?,
        )?)),
    }
}

/// The one accepted `photos processing-recipe save --input` document shape.
/// Derived decoding rejects unknown keys, duplicate keys, trailing content,
/// and non-object documents everywhere except inside the module-owned
/// parameter trees, which the CLI preserves verbatim.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveProcessingRecipeInput {
    pub(super) request_id: String,
    #[serde(deserialize_with = "required_nullable_string")]
    pub(super) expected_recipe_revision: Option<String>,
    pub(super) expected_source_revision: String,
    #[serde(deserialize_with = "required_nullable_string")]
    pub(super) current_step_id: Option<String>,
    pub(super) steps: Vec<ProcessingStepWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RebindProcessingRecipeInput {
    request_id: String,
    expected_recipe_revision: String,
    new_source_revision: String,
}

pub(super) fn parse_processing_rebind(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: RebindProcessingRecipeInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid("input", "The input must contain exactly requestId, expectedRecipeRevision, and newSourceRevision.")
    })?;
    validate_request_identity(&input.request_id)?;
    validate_nonempty_revision(
        "expectedRecipeRevision",
        Some(&input.expected_recipe_revision),
    )?;
    validate_source_revision("newSourceRevision", &input.new_source_revision)?;
    serde_json::to_value(input).map_err(|_| unusable_input())
}

/// One Processing Step record: an opaque step identity unique within the
/// recipe, one selected module, one explicit input binding, and one complete
/// module-owned parameter snapshot.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessingStepWire {
    pub(super) step_id: String,
    pub(super) module: String,
    pub(super) input: ProcessingInputWire,
    pub(super) parameters: ProcessingParametersWire,
}

/// One complete, versioned, module-owned parameter snapshot. The tree is an
/// opaque value: the CLI neither flattens it into a shared map, merges
/// schemas across modules, nor interprets unknown fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessingParametersWire {
    pub(super) schema_version: String,
    pub(super) tree: Value,
}

/// The one explicit input binding of a Processing Step: the guarded Original
/// identity of a Photo, or one published immutable Processing Artifact with
/// the concrete image contract its bytes carry.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ProcessingInputWire {
    #[serde(rename_all = "camelCase")]
    Original {
        photo_id: String,
        source_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    Artifact {
        artifact_id: String,
        contract: ProcessingContractWire,
    },
}

/// The concrete image contract an artifact input declares: module-owned
/// format, precision, color space, transfer, and encoding names around an
/// explicit finite geometry.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessingContractWire {
    pub(super) format: String,
    pub(super) precision: String,
    pub(super) color_space: String,
    pub(super) transfer: String,
    pub(super) geometry: ProcessingGeometryWire,
    pub(super) encoding: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessingGeometryWire {
    pub(super) width: u32,
    pub(super) height: u32,
}

/// Validates the composable save body in the service's own admission order:
/// request identity, revision guards, the selected current step, then every
/// step's bounded identity, input binding, and parameter snapshot bounds.
/// Module admission — discovery, parameter versions, artifact leases — stays
/// the service's decision.
pub(super) fn parse_processing_save(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: SaveProcessingRecipeInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly requestId, \
             expectedRecipeRevision, expectedSourceRevision, currentStepId, and steps, carrying \
             the composable Processing Step wire shapes.",
        )
    })?;
    validate_request_identity(&input.request_id)?;
    validate_nonempty_revision(
        "expectedRecipeRevision",
        input.expected_recipe_revision.as_deref(),
    )?;
    validate_source_revision("expectedSourceRevision", &input.expected_source_revision)?;
    validate_nonempty_revision("currentStepId", input.current_step_id.as_deref())?;
    if input.steps.len() > MAXIMUM_RECIPE_STEPS {
        return Err(CommandFailure::invalid(
            "steps",
            format!("The recipe admits at most {MAXIMUM_RECIPE_STEPS} Processing Steps."),
        ));
    }
    let mut seen = HashSet::with_capacity(input.steps.len());
    for step in &input.steps {
        validate_bounded_argument("stepId", &step.step_id, MAXIMUM_STEP_ID_BYTES)?;
        validate_bounded_argument("module", &step.module, MAXIMUM_MODULE_ID_BYTES)?;
        if !seen.insert(step.step_id.as_str()) {
            return Err(CommandFailure::invalid(
                "steps",
                "Each stepId must be unique within the recipe.",
            ));
        }
        match &step.input {
            ProcessingInputWire::Original {
                photo_id,
                source_revision,
            } => {
                validate_bounded_argument("photoId", photo_id, MAXIMUM_PHOTO_ID_BYTES)?;
                validate_source_revision("sourceRevision", source_revision)?;
            }
            ProcessingInputWire::Artifact {
                artifact_id,
                contract,
            } => {
                validate_bounded_argument("artifactId", artifact_id, MAXIMUM_ARTIFACT_ID_BYTES)?;
                for (argument, name) in [
                    ("format", &contract.format),
                    ("precision", &contract.precision),
                    ("colorSpace", &contract.color_space),
                    ("transfer", &contract.transfer),
                    ("encoding", &contract.encoding),
                ] {
                    validate_bounded_argument(argument, name, MAXIMUM_CONTRACT_NAME_BYTES)?;
                }
                let geometry = &contract.geometry;
                if geometry.width == 0
                    || geometry.height == 0
                    || geometry.width > MAXIMUM_GEOMETRY_EDGE
                    || geometry.height > MAXIMUM_GEOMETRY_EDGE
                {
                    return Err(CommandFailure::invalid(
                        "geometry",
                        "Each geometry edge must be 1 through 65536 pixels.",
                    ));
                }
            }
        }
        validate_bounded_argument(
            "schemaVersion",
            &step.parameters.schema_version,
            MAXIMUM_CONTRACT_NAME_BYTES,
        )?;
        if parameter_tree_depth(&step.parameters.tree) > MAXIMUM_PARAMETER_TREE_DEPTH {
            return Err(CommandFailure::invalid(
                "tree",
                "The module-owned parameter tree nests at most 32 levels deep.",
            ));
        }
    }
    match &input.current_step_id {
        Some(current) if !seen.contains(current.as_str()) => Err(CommandFailure::invalid(
            "currentStepId",
            "The currentStepId must name one of the recipe's steps.",
        )),
        None if !input.steps.is_empty() => Err(CommandFailure::invalid(
            "currentStepId",
            "A recipe with steps must select exactly one current step.",
        )),
        _ => serde_json::to_value(&input).map_err(|_| unusable_input()),
    }
}

/// One bounded identifier of the composable wire contract: nonempty, free of
/// control characters, equal to its trimmed form, and no longer than the
/// published bound. Opaque does not mean unbounded.
pub(super) fn validate_bounded_argument(
    argument: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), CommandFailure> {
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value != value.trim()
        || value.len() > maximum
    {
        return Err(CommandFailure::invalid(
            argument,
            format!(
                "The value must be 1 through {maximum} bytes without control or surrounding \
                 whitespace characters."
            ),
        ));
    }
    Ok(())
}

/// One bounded opaque recipe revision.
pub(super) fn bounded_revision(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAXIMUM_REVISION_BYTES
}

pub(super) fn bounded_source_revision(value: &str) -> bool {
    !value.is_empty() && value.len() <= crate::MAXIMUM_SOURCE_REVISION_BYTES
}

/// The nesting depth of one module-owned parameter tree, measured
/// iteratively so no hostile depth can exhaust the stack while it is
/// measured.
pub(super) fn parameter_tree_depth(tree: &Value) -> usize {
    let mut depth = 0;
    let mut level = vec![tree];
    while !level.is_empty() {
        depth += 1;
        let mut next = Vec::new();
        for value in level {
            match value {
                Value::Array(items) => next.extend(items),
                Value::Object(fields) => next.extend(fields.values()),
                _ => {}
            }
        }
        level = next;
    }
    depth
}

/// Executes one composable recipe command. Reads report the validated
/// recipe verbatim plus the Photo Destination `webUrl`; the save submits the
/// exact prepared body once, and anything unusable in a post-admission
/// response stays an unknown outcome. No automatic pre-read replaces the
/// caller's guards and no refused or lost write is retried.
pub(crate) async fn execute_processing(
    client: &ServiceClient,
    admission: &AdmissionState,
    command: &ProcessingRecipeCommand,
    prepared: Option<Value>,
) -> Result<Value, CommandFailure> {
    match command {
        ProcessingRecipeCommand::Get { photo_id } => processing_recipe_get(client, photo_id).await,
        ProcessingRecipeCommand::Save(args) => {
            let body = prepared.ok_or_else(unusable_input)?;
            processing_recipe_write(client, admission, args, body, false).await
        }
        ProcessingRecipeCommand::Rebind(args) => {
            let body = prepared.ok_or_else(unusable_input)?;
            processing_recipe_write(client, admission, args, body, true).await
        }
    }
}

async fn processing_recipe_get(
    client: &ServiceClient,
    photo_id: &str,
) -> Result<Value, CommandFailure> {
    let read: ProcessingRecipeReadWire = client
        .json(
            PROCESSING_GET_OPERATION,
            Method::GET,
            client.endpoint(&["api", "photos", photo_id, "processing-recipe"]),
            None,
        )
        .await?;
    validated_processing_recipe_read(read, photo_id, &client.origin)
}

/// Submits one guarded composable save with the exact prepared body. The
/// service assigns the committed recipe revision, so the confirmed response
/// must carry a closed outcome, a recipe inside the closed shapes, and
/// exactly the guarded source revision; anything else is an unknown outcome.
async fn processing_recipe_write(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &ProcessingRecipeWriteArgs,
    body: Value,
    rebind: bool,
) -> Result<Value, CommandFailure> {
    let prepared_field = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let Some(request_id) = prepared_field("requestId") else {
        return Err(unusable_input());
    };
    let Some(submitted_source) = prepared_field(if rebind {
        "newSourceRevision"
    } else {
        "expectedSourceRevision"
    }) else {
        return Err(unusable_input());
    };
    let identity = MutationIdentity {
        operation: if rebind {
            PROCESSING_REBIND_OPERATION
        } else {
            PROCESSING_SAVE_OPERATION
        },
        photo_ids: vec![args.photo_id.clone()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let (status, bytes) = client
        .mutation_statuses(
            Method::POST,
            &identity,
            admission,
            if rebind {
                client.endpoint(&[
                    "api",
                    "photos",
                    &args.photo_id,
                    "processing-recipe",
                    "rebind",
                ])
            } else {
                client.endpoint(&["api", "photos", &args.photo_id, "processing-recipe"])
            },
            Some(body),
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await?;
    let result: ProcessingRecipeWriteWire =
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))?;
    confirmed_processing_recipe_write(
        &identity,
        &args.photo_id,
        &request_id,
        &submitted_source,
        status,
        result,
        &client.origin,
    )
}

/// Validates one confirmed composable write against the submitted request.
/// `saved` arrives as Created and `replayed` or `unchanged` as OK, the recipe
/// repeats the Photo identity and closed step shapes, the reported recipe
/// version is the committed recipe's own revision, and the returned source
/// revision is exactly the one this command submitted.
pub(super) fn confirmed_processing_recipe_write(
    identity: &MutationIdentity,
    photo_id: &str,
    request_id: &str,
    submitted_source: &str,
    status: StatusCode,
    result: ProcessingRecipeWriteWire,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let outcome_valid = match result.outcome.as_str() {
        "saved" => status == StatusCode::CREATED,
        "replayed" | "unchanged" => status == StatusCode::OK,
        _ => false,
    };
    if !outcome_valid
        || result.recipe_version.is_empty()
        || result.recipe_version != result.recipe.revision
        || result.source_revision != submitted_source
        || !processing_recipe_valid(&result.recipe, photo_id)
    {
        return Err(CommandFailure::unknown(identity));
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::unknown(identity))?;
    Ok(json!({
        "photoId": photo_id,
        "requestId": request_id,
        "outcome": result.outcome,
        "recipe": serde_json::to_value(&result.recipe)
            .map_err(|_| CommandFailure::unknown(identity))?,
        "recipeVersion": result.recipe_version,
        "sourceRevision": result.source_revision,
        "webUrl": web_url,
    }))
}

// ------------------------------------------------ composable wire shapes

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProcessingRecipeReadWire {
    pub(super) photo_id: String,
    /// The observed source revision: empty exactly when no revision is
    /// currently published, never a synthesized one.
    pub(super) source_revision: String,
    #[serde(deserialize_with = "required_nullable")]
    pub(super) recipe: Option<ProcessingRecipeWire>,
}

/// One retained composable recipe: the committed revision, the guarded
/// source revision it is bound to, zero or more steps, and the caller's
/// selected current step.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProcessingRecipeWire {
    pub(super) photo_id: String,
    pub(super) revision: String,
    pub(super) source_revision: String,
    #[serde(deserialize_with = "required_nullable_string")]
    pub(super) current_step_id: Option<String>,
    pub(super) steps: Vec<ProcessingStepWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProcessingRecipeWriteWire {
    pub(super) outcome: String,
    pub(super) recipe: ProcessingRecipeWire,
    pub(super) recipe_version: String,
    pub(super) source_revision: String,
}

/// Validates one retained composable recipe against the closed wire shapes:
/// bounded identities, a finite step set with unique step ids, closed input
/// bindings and parameter snapshots, and a current step that is one of the
/// recipe's steps whenever steps exist. The module-owned trees stay opaque.
pub(super) fn processing_recipe_valid(recipe: &ProcessingRecipeWire, photo_id: &str) -> bool {
    let bounded_name = |value: &str, maximum: usize| {
        !value.is_empty()
            && value.len() <= maximum
            && !value.chars().any(char::is_control)
            && value == value.trim()
    };
    recipe.photo_id == photo_id
        && bounded_revision(&recipe.revision)
        && bounded_source_revision(&recipe.source_revision)
        && recipe.steps.len() <= MAXIMUM_RECIPE_STEPS
        && recipe.steps.iter().all(|step| {
            bounded_name(&step.step_id, MAXIMUM_STEP_ID_BYTES)
                && bounded_name(&step.module, MAXIMUM_MODULE_ID_BYTES)
                && bounded_name(&step.parameters.schema_version, MAXIMUM_CONTRACT_NAME_BYTES)
                && match &step.input {
                    ProcessingInputWire::Original {
                        photo_id,
                        source_revision,
                    } => {
                        bounded_name(photo_id, MAXIMUM_PHOTO_ID_BYTES)
                            && bounded_source_revision(source_revision)
                    }
                    ProcessingInputWire::Artifact {
                        artifact_id,
                        contract,
                    } => {
                        bounded_name(artifact_id, MAXIMUM_ARTIFACT_ID_BYTES)
                            && bounded_name(&contract.format, MAXIMUM_CONTRACT_NAME_BYTES)
                            && bounded_name(&contract.precision, MAXIMUM_CONTRACT_NAME_BYTES)
                            && bounded_name(&contract.color_space, MAXIMUM_CONTRACT_NAME_BYTES)
                            && bounded_name(&contract.transfer, MAXIMUM_CONTRACT_NAME_BYTES)
                            && bounded_name(&contract.encoding, MAXIMUM_CONTRACT_NAME_BYTES)
                            && contract.geometry.width > 0
                            && contract.geometry.height > 0
                            && contract.geometry.width <= MAXIMUM_GEOMETRY_EDGE
                            && contract.geometry.height <= MAXIMUM_GEOMETRY_EDGE
                    }
                }
        })
        && recipe
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<HashSet<_>>()
            .len()
            == recipe.steps.len()
        && match &recipe.current_step_id {
            Some(current) => {
                bounded_name(current, MAXIMUM_STEP_ID_BYTES)
                    && recipe.steps.iter().any(|step| step.step_id == *current)
            }
            None => recipe.steps.is_empty(),
        }
}

/// Validates one composable recipe read against the closed wire contract
/// and renders the CLI result with the added Photo Destination `webUrl`. An
/// absent recipe is a successful read with `recipe: null`, not a saved
/// baseline; a retained recipe keeps its source binding even when the currently
/// observed source differs. A response outside the contract is a transport failure.
pub(super) fn validated_processing_recipe_read(
    read: ProcessingRecipeReadWire,
    photo_id: &str,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let invalid = || CommandFailure::transport(PROCESSING_GET_OPERATION);
    let recipe_valid = match &read.recipe {
        None => true,
        Some(recipe) => processing_recipe_valid(recipe, photo_id),
    };
    if read.photo_id != photo_id
        || !recipe_valid
        || (!read.source_revision.is_empty() && !bounded_source_revision(&read.source_revision))
    {
        return Err(invalid());
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}")).map_err(|()| invalid())?;
    let recipe = read
        .recipe
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| invalid())?;
    Ok(json!({
        "photoId": photo_id,
        "sourceRevision": read.source_revision,
        "recipe": recipe,
        "webUrl": web_url,
    }))
}
