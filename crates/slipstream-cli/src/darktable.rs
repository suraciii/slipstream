use super::*;
use clap::{Arg, ArgAction, ArgMatches, FromArgMatches};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
mod download;
pub(crate) use download::download;

#[derive(Debug)]
pub struct DarktableArgs {
    pub(crate) operation: String,
    pub(crate) matches: ArgMatches,
}
impl Args for DarktableArgs {
    fn augment_args(command: clap::Command) -> clap::Command {
        command
            .subcommands(grammar().get_subcommands().cloned())
            .subcommand_required(true)
    }
    fn augment_args_for_update(command: clap::Command) -> clap::Command {
        Self::augment_args(command)
    }
}
impl FromArgMatches for DarktableArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let mut current = matches;
        let mut path = Vec::new();
        while let Some((name, child)) = current.subcommand() {
            path.push(name);
            current = child;
        }
        Ok(Self {
            operation: path.join(" "),
            matches: current.clone(),
        })
    }
    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}
fn option(name: &'static str) -> Arg {
    Arg::new(name).long(name).num_args(1)
}
fn positional(name: &'static str) -> Arg {
    Arg::new(name).required(true)
}
fn transform(command: &clap::Command, path: &str) -> clap::Command {
    let mut result = clap::Command::new(command.get_name().to_owned())
        .about(
            command
                .get_about()
                .cloned()
                .unwrap_or_else(|| "Semantic darktable operation".into()),
        )
        .infer_long_args(false);
    let args: Vec<_> = command
        .get_arguments()
        .filter(|arg| {
            !matches!(arg.get_id().as_str(), "output" | "help")
                && !(matches!(path, "preview render" | "export create")
                    && matches!(arg.get_id().as_str(), "directory" | "name"))
                && !(path == "export state-file" && arg.get_id() == "file")
        })
        .cloned()
        .collect();
    let scoped = command.get_arguments().any(|arg| arg.get_id() == "state");
    let targeted = command.get_arguments().any(|arg| arg.get_id() == "target");
    for arg in args {
        result = result.arg(if arg.get_id() == "target" {
            arg.value_name("PHOTO_ID")
        } else if arg.get_id() == "state" {
            arg.long("photo").value_name("PHOTO_ID")
        } else {
            arg
        });
    }
    if scoped || targeted {
        let mut edit = option("edit").requires(if scoped { "state" } else { "target" });
        if command.get_arguments().any(|arg| arg.get_id() == "cursor") {
            edit = edit.conflicts_with("cursor");
        }
        result = result.arg(edit);
    }
    let children: Vec<_> = command
        .get_subcommands()
        .filter(|child| !matches!(child.get_name(), "open" | "__invoke" | "__inspect"))
        .collect();
    if !children.is_empty() {
        result = result.subcommand_required(true);
    }
    for child in children {
        let next = if path.is_empty() {
            child.get_name().to_owned()
        } else {
            format!("{path} {}", child.get_name())
        };
        result = result.subcommand(transform(child, &next));
    }
    result
}
pub(crate) fn grammar() -> clap::Command {
    let mut command = transform(&darktable_edit::command::grammar(), "");
    command = command.mut_subcommand("edit", |cmd| {
        let create = clap::Command::new("create")
            .arg(positional("target"))
            .arg(option("input"))
            .arg(option("if-state"))
            .arg(option("request"));
        cmd.subcommand(create)
    });
    command = command.mut_subcommand("preview", |mut cmd| {
        for name in ["status", "wait", "download"] {
            let mut leaf = clap::Command::new(name).arg(positional("id"));
            if name == "download" {
                leaf = leaf.arg(option("file").required(true)).arg(
                    Arg::new("comparison")
                        .long("comparison")
                        .action(ArgAction::SetTrue),
                );
            }
            cmd = cmd.subcommand(leaf);
        }
        cmd.subcommand(
            clap::Command::new("proxy")
                .subcommand_required(true)
                .subcommand(clap::Command::new("get").arg(positional("target")))
                .subcommand(
                    clap::Command::new("create")
                        .arg(positional("target"))
                        .arg(option("input").required(true))
                        .arg(option("if-state"))
                        .arg(option("request")),
                )
                .subcommand(
                    clap::Command::new("remove")
                        .arg(positional("target"))
                        .arg(option("if-state"))
                        .arg(option("request")),
                ),
        )
    });
    command.mut_subcommand("export", |mut cmd| {
        let list = clap::Command::new("list")
            .arg(positional("target"))
            .arg(option("edit"))
            .arg(option("limit").value_parser(clap::value_parser!(u64).range(1..=60)));
        cmd = cmd.subcommand(list);
        for name in ["status", "wait", "cancel", "retry"] {
            let mut leaf = clap::Command::new(name)
                .arg(positional("target"))
                .arg(positional("id"));
            if matches!(name, "cancel" | "retry") {
                leaf = leaf.arg(option("request"));
            }
            cmd = cmd.subcommand(leaf);
        }
        for name in ["download", "state-file-download", "historical-download"] {
            cmd = cmd.subcommand(
                clap::Command::new(name)
                    .arg(positional("id"))
                    .arg(option("file").required(true)),
            );
        }
        cmd
    })
}
const OPERATIONS: &[&str] = &[
    "darktable-modules-list",
    "darktable-modules-describe",
    "darktable-edit-create",
    "darktable-edit-show",
    "darktable-edit-set",
    "darktable-edit-validate",
    "darktable-edit-apply",
    "darktable-edit-reset",
    "darktable-edit-rebind",
    "darktable-edit-instances-list",
    "darktable-edit-instances-show",
    "darktable-edit-instances-add",
    "darktable-edit-instances-clone",
    "darktable-edit-instances-enable",
    "darktable-edit-instances-remove",
    "darktable-edit-instances-reorder",
    "darktable-edit-masks-list",
    "darktable-edit-masks-show",
    "darktable-edit-masks-create",
    "darktable-edit-masks-set",
    "darktable-edit-masks-remove",
    "darktable-edit-actions-list",
    "darktable-edit-actions-describe",
    "darktable-edit-actions-run",
    "darktable-edit-output-show",
    "darktable-edit-output-set",
    "darktable-edit-migration-show",
    "darktable-edit-migrate",
    "darktable-resources-kinds",
    "darktable-resources-list",
    "darktable-resources-show",
    "darktable-resources-import",
    "darktable-resources-remove",
    "darktable-presets-list",
    "darktable-presets-show",
    "darktable-presets-save",
    "darktable-presets-apply",
    "darktable-presets-import",
    "darktable-presets-export",
    "darktable-presets-remove",
    "darktable-styles-list",
    "darktable-styles-show",
    "darktable-styles-save",
    "darktable-styles-apply",
    "darktable-styles-import",
    "darktable-styles-export",
    "darktable-styles-remove",
    "darktable-preview-render",
    "darktable-preview-status",
    "darktable-preview-wait",
    "darktable-preview-download",
    "darktable-preview-proxy-get",
    "darktable-preview-proxy-create",
    "darktable-preview-proxy-remove",
    "darktable-export-create",
    "darktable-export-list",
    "darktable-export-status",
    "darktable-export-wait",
    "darktable-export-cancel",
    "darktable-export-retry",
    "darktable-export-download",
    "darktable-export-state-file",
    "darktable-export-state-file-download",
    "darktable-export-historical-download",
    "darktable-history-list",
    "darktable-history-show",
    "darktable-history-restore",
];
impl DarktableArgs {
    pub(crate) fn operation(&self) -> Operation {
        let wire = format!("darktable-{}", self.operation.replace(' ', "-"));
        Operation::Darktable(
            OPERATIONS
                .iter()
                .copied()
                .find(|item| *item == wire)
                .expect("grammar operation registered"),
        )
    }
}
pub(crate) fn tool_failure(error: darktable_edit::protocol::ToolError) -> CommandFailure {
    CommandFailure::from_payload(
        error.exit_code,
        ErrorPayload {
            code: error.code,
            message: error.message,
            effect: error.effect,
            details: error.details,
        },
    )
}
pub(crate) struct Prepared {
    pub photo: Option<String>,
    pub edit: Option<String>,
    pub arguments: Value,
    pub destination: Option<preview_download::Destination>,
    pub upload: Option<Vec<u8>>,
    pub filename: Option<String>,
}
fn string(matches: &ArgMatches, key: &str) -> Option<String> {
    matches.try_get_one::<String>(key).ok().flatten().cloned()
}
pub(crate) fn prepare(command: &DarktableArgs) -> Result<Prepared, CommandFailure> {
    let mut matches = command.matches.clone();
    let photo = string(&matches, "target").or_else(|| string(&matches, "state"));
    let edit = string(&matches, "edit");
    let id = string(&matches, "id");
    for (key, value) in [
        ("photo", photo.as_deref()),
        ("edit", edit.as_deref()),
        ("id", id.as_deref()),
    ] {
        if value.is_some_and(|value| !valid_request_identity(value)) {
            return Err(CommandFailure::invalid(
                key,
                "Identifiers must use 1–128 ASCII letters, digits, dot, underscore or hyphen.",
            ));
        }
    }
    let _ = matches.try_remove_one::<String>("state");
    let _ = matches.try_remove_one::<String>("edit");
    let integration = matches!(
        command.operation.as_str(),
        "edit create"
            | "edit rebind"
            | "preview render"
            | "export state-file"
            | "export list"
            | "preview status"
            | "preview wait"
            | "preview download"
            | "preview proxy get"
            | "preview proxy create"
            | "preview proxy remove"
            | "export status"
            | "export wait"
            | "export cancel"
            | "export retry"
            | "export download"
            | "export state-file-download"
            | "export historical-download"
    );
    let mut arguments = if integration {
        let mut args = serde_json::Map::new();
        for key in ["id", "revision", "request", "if-state", "input"] {
            if let Some(value) = string(&matches, key) {
                let normalized = match key {
                    "request" => "requestId",
                    "if-state" => "ifState",
                    _ => key,
                };
                let value = if key == "input" {
                    darktable_edit::command::read_command_input(
                        &value,
                        &mut std::io::stdin().lock(),
                    )
                    .map_err(tool_failure)?
                } else {
                    json!(value)
                };
                args.insert(normalized.into(), value);
            }
        }
        if let Ok(Some(limit)) = matches.try_get_one::<u64>("limit") {
            args.insert("limit".into(), json!(limit));
        }
        if matches.try_get_one::<bool>("comparison").ok().flatten() == Some(&true) {
            args.insert("comparison".into(), json!(true));
        }
        Value::Object(args)
    } else {
        darktable_edit::command::normalize_matches(
            command.operation.clone(),
            &matches,
            &mut std::io::stdin().lock(),
        )
        .map_err(tool_failure)?
        .arguments
    };
    for key in ["requestId", "revision"] {
        if arguments
            .get(key)
            .is_some_and(|value| !value.as_str().is_some_and(valid_request_identity))
        {
            return Err(CommandFailure::invalid(key, "Invalid identifier."));
        }
    }
    if arguments.get("ifState").is_some_and(|value| {
        !value.as_str().is_some_and(|token| {
            !token.is_empty()
                && token.len() <= 512
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
    }) {
        return Err(CommandFailure::invalid("ifState", "Invalid state token."));
    }
    if command.operation == "preview proxy create" {
        let valid = arguments["input"].as_object().is_some_and(|fields| {
            fields.len() == 1
                && fields
                    .get("expectedSourceRevision")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES
                    })
        });
        if !valid {
            return Err(CommandFailure::invalid(
                "input",
                "Expected exactly one bounded expectedSourceRevision.",
            ));
        }
    }
    if command.operation == "preview render"
        && let Some(input) = arguments.get("input")
    {
        let fields = input
            .as_object()
            .filter(|fields| {
                fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "comparison" | "region"))
            })
            .ok_or_else(|| {
                CommandFailure::invalid("input", "Expected comparison/region preview options.")
            })?;
        if fields
            .get("region")
            .is_some_and(|region| !region.is_null() && !region.is_object())
        {
            return Err(CommandFailure::invalid(
                "region",
                "Region must be an object or null.",
            ));
        }
        if let Some(comparison) = fields.get("comparison")
            && !comparison.is_null()
            && comparison != "baseline"
        {
            let compared = comparison
                .as_object()
                .filter(|fields| {
                    !fields.is_empty()
                        && fields
                            .keys()
                            .all(|key| matches!(key.as_str(), "editId" | "revision"))
                })
                .ok_or_else(|| {
                    CommandFailure::invalid(
                        "comparison",
                        "Expected revision and optional comparison editId.",
                    )
                })?;
            if !compared
                .values()
                .all(|value| value.as_str().is_some_and(valid_request_identity))
            {
                return Err(CommandFailure::invalid(
                    "comparison",
                    "Invalid comparison identity.",
                ));
            }
        }
    }
    if matches!(command.operation.as_str(), "edit create" | "edit rebind")
        && let Some(input) = arguments.get("input")
    {
        let fields = input
            .as_object()
            .filter(|fields| fields.len() == 1 && fields.contains_key("input"))
            .and_then(|fields| fields["input"].as_object())
            .ok_or_else(|| {
                CommandFailure::invalid("input", "Expected exactly one typed input binding.")
            })?;
        let valid = match fields.get("kind").and_then(Value::as_str) {
            Some("workspace-artifact") => {
                fields.len() == 3
                    && ["artifactId", "revision"].iter().all(|key| {
                        fields
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(valid_request_identity)
                    })
            }
            Some("original") => {
                fields.len() == 2
                    && fields
                        .get("sourceRevision")
                        .and_then(Value::as_str)
                        .is_some_and(|value| {
                            !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES
                        })
            }
            _ => false,
        };
        if !valid {
            return Err(CommandFailure::invalid(
                "input",
                "Invalid workspace-artifact or original binding.",
            ));
        }
    }
    let file = string(&matches, "file");
    let importing = command.operation.ends_with(" import");
    let exporting =
        command.operation.ends_with(" export") || command.operation.ends_with("download");
    let destination = if exporting {
        Some(preview_download::Destination::preflight(
            preview_download::DestinationKind::ProcessingArtifact,
            Path::new(file.as_deref().expect("file required")),
        )?)
    } else {
        None
    };
    let upload = if importing {
        use std::os::unix::fs::OpenOptionsExt;
        let path = file.as_deref().expect("import file");
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| CommandFailure::invalid("file", "Cannot open regular import file."))?;
        if !file
            .metadata()
            .map_err(|_| CommandFailure::invalid("file", "Cannot inspect import file."))?
            .is_file()
        {
            return Err(CommandFailure::invalid(
                "file",
                "Import must be a regular nonsymlink file.",
            ));
        }
        let observation = photo_files::Observation::of(&file)
            .map_err(|_| CommandFailure::invalid("file", "Cannot inspect import bytes."))?;
        let maximum = if command.operation == "resources import" {
            64 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| CommandFailure::invalid("file", "Cannot read import file."))?;
        observation.verify(&file).map_err(|_| {
            CommandFailure::invalid("file", "Import bytes changed during validation.")
        })?;
        if bytes.is_empty() || bytes.len() as u64 > maximum {
            return Err(CommandFailure::invalid(
                "file",
                "Import is empty or exceeds its qualified byte bound.",
            ));
        }
        if command.operation != "resources import" {
            let value = photo_files::read_json(bytes.as_slice(), maximum as usize)
                .map_err(|error| CommandFailure::invalid("file", error.to_string()))?;
            darktable_edit::command::validate_interchange(
                &value,
                if command.operation == "presets import" {
                    "preset"
                } else {
                    "style"
                },
            )
            .map_err(tool_failure)?;
        }
        Some(bytes)
    } else {
        None
    };
    if command.operation.ends_with(" export") {
        let filename = Path::new(file.as_deref().unwrap())
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CommandFailure::invalid("file", "Invalid output filename."))?;
        arguments["file"] = json!({"filename":filename});
    } else {
        arguments.as_object_mut().unwrap().remove("file");
    }
    let filename = file
        .as_deref()
        .and_then(|file| Path::new(file).file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    if filename.as_ref().is_some_and(|name| {
        name.is_empty() || name.len() > 255 || name.chars().any(char::is_control)
    }) {
        return Err(CommandFailure::invalid(
            "file",
            "Invalid transfer filename.",
        ));
    }
    Ok(Prepared {
        photo,
        edit,
        arguments,
        destination,
        upload,
        filename,
    })
}

fn endpoint(client: &ServiceClient, photo: Option<&str>, tail: &[&str]) -> Url {
    let mut segments = vec!["api"];
    if let Some(photo) = photo {
        segments.extend(["photos", photo]);
    }
    segments.push("darktable");
    segments.extend_from_slice(tail);
    client.endpoint(&segments)
}
pub(crate) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    publication: &PublicationState,
    command: &DarktableArgs,
    mut prepared: Prepared,
) -> Result<Value, CommandFailure> {
    let operation = command.operation();
    if let Some(bytes) = prepared.upload.take() {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let response = client
            .client
            .post(endpoint(client, prepared.photo.as_deref(), &["files"]))
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&client.token)
            .header(
                "X-Filename",
                prepared.filename.as_deref().unwrap_or("import"),
            )
            .header("X-Sha256", sha256)
            .timeout(client.control_timeout)
            .body(bytes)
            .send()
            .await
            .map_err(|_| CommandFailure::transport(operation))?;
        let status = response.status();
        let bytes = response_bytes(response, operation).await?;
        if status != StatusCode::OK && status != StatusCode::CREATED {
            return Err(transfer_failure(status, &bytes, operation, &client.token));
        }
        let handle: Value =
            serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(operation))?;
        let id = handle["transferId"]
            .as_str()
            .filter(|id| valid_request_identity(id))
            .ok_or_else(|| CommandFailure::transport(operation))?;
        prepared.arguments["file"] = json!({"transferId":id});
    }
    let op = command.operation.as_str();
    let id = prepared.arguments["id"].as_str().unwrap_or("").to_owned();
    if op == "export cancel" {
        let identity = MutationIdentity {
            operation,
            photo_ids: prepared.photo.iter().cloned().collect(),
            album_id: None,
            album_name: None,
            mappings: vec![],
        };
        let mut body = json!({});
        if let Some(request) = prepared.arguments.get("requestId") {
            body["requestId"] = request.clone();
        }
        return client
            .mutation(
                &identity,
                admission,
                endpoint(client, prepared.photo.as_deref(), &["jobs", &id, "cancel"]),
                body,
            )
            .await;
    }
    if op.ends_with("download") {
        let kind = match op {
            "preview download" => "previews",
            "export state-file-download" => "state-files",
            "export historical-download" => "historical",
            _ => "artifacts",
        };
        let mut url = endpoint(client, None, &[kind, &id, "bytes"]);
        if prepared.arguments["comparison"] == true {
            url.query_pairs_mut().append_pair("comparison", "true");
        }
        return download(
            client,
            operation,
            url,
            prepared.destination.expect("download destination"),
            publication,
            None,
        )
        .await;
    }
    if matches!(
        op,
        "preview status" | "preview wait" | "export status" | "export wait"
    ) {
        let tail = if op.starts_with("preview") {
            vec!["previews", id.as_str()]
        } else {
            vec!["jobs", id.as_str()]
        };
        let url = endpoint(client, prepared.photo.as_deref(), &tail);
        let wait_deadline = tokio::time::Instant::now() + client.control_timeout;
        loop {
            let result: Value = client
                .json(operation, Method::GET, url.clone(), None)
                .await?;
            let status = result["status"]
                .as_str()
                .or_else(|| result["state"].as_str())
                .ok_or_else(|| CommandFailure::transport(operation))?;
            if !matches!(status, "queued" | "running" | "accepted" | "executing") {
                if matches!(status, "failed" | "cancelled" | "unknown") {
                    return Err(CommandFailure {
                        exit_code: if status == "unknown" { 7 } else { 6 },
                        payload: ErrorPayload {
                            code: if status == "unknown" {
                                "outcome_unknown"
                            } else {
                                "execution_failed"
                            }
                            .into(),
                            message: "Inspect the retained captured job before dependent work."
                                .into(),
                            effect: if status == "unknown" {
                                "unknown"
                            } else {
                                "none"
                            }
                            .into(),
                            details: json!({"operation":operation.wire(),"requestId":id}),
                        },
                        data: Some(Box::new(result)),
                    });
                }
                if !matches!(status, "succeeded" | "ready" | "published") {
                    return Err(CommandFailure::transport(operation));
                }
                return Ok(result);
            }
            if !op.ends_with("wait") {
                return Ok(result);
            }
            if tokio::time::Instant::now() + Duration::from_millis(100) >= wait_deadline {
                return Err(CommandFailure {
                    exit_code: 6,
                    payload: ErrorPayload {
                        code: "wait_timeout".into(),
                        message: "The captured job remains pending; query its status.".into(),
                        effect: "none".into(),
                        details: json!({"operation":operation.wire(),"requestId":id}),
                    },
                    data: Some(Box::new(result)),
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let read = op.ends_with(" list")
        || op.ends_with(" show")
        || op.ends_with(" describe")
        || op.ends_with(" kinds")
        || op == "edit validate"
        || op == "preview proxy get";
    let url = endpoint(client, prepared.photo.as_deref(), &["commands"]);
    let body = json!({"operation":op,"editId":prepared.edit,"arguments":prepared.arguments});
    let mut result: Value = if read {
        client
            .json(operation, Method::POST, url, Some(body))
            .await?
    } else {
        let identity = MutationIdentity {
            operation,
            photo_ids: prepared.photo.iter().cloned().collect(),
            album_id: None,
            album_name: None,
            mappings: vec![],
        };
        client
            .mutation_admitting(
                &identity,
                admission,
                url,
                body.clone(),
                &[StatusCode::OK, StatusCode::CREATED, StatusCode::ACCEPTED],
            )
            .await
            .map_err(|mut failure| {
                if failure.payload.effect == "unknown" {
                    failure.payload.details["requestId"] = body["arguments"]["requestId"].clone();
                    failure.payload.details["command"] = body.clone();
                }
                failure
            })?
    };
    if !result.is_object()
        || prepared
            .photo
            .as_ref()
            .is_some_and(|photo| result.get("photoId").is_some_and(|id| id != &json!(photo)))
    {
        if read {
            return Err(CommandFailure::transport(operation));
        }
        return Err(CommandFailure::unknown(&MutationIdentity {
            operation,
            photo_ids: prepared.photo.iter().cloned().collect(),
            album_id: None,
            album_name: None,
            mappings: vec![],
        }));
    }
    if let Some(destination) = prepared.destination {
        let transfer = result["transferId"]
            .as_str()
            .filter(|id| valid_request_identity(id))
            .ok_or_else(|| CommandFailure::transport(operation))?;
        let output = download(
            client,
            operation,
            endpoint(
                client,
                prepared.photo.as_deref(),
                &["files", transfer, "bytes"],
            ),
            destination,
            publication,
            Some(&result),
        )
        .await?;
        result["download"] = output;
    }
    Ok(result)
}
fn transfer_failure(
    status: StatusCode,
    bytes: &[u8],
    operation: Operation,
    token: &str,
) -> CommandFailure {
    access_boundary_failure(status, None, bytes, operation)
        .or_else(|| {
            serde_json::from_slice::<ErrorResponse>(bytes)
                .ok()
                .and_then(|error| validated_route_failure(error.error, operation, token))
        })
        .unwrap_or_else(|| CommandFailure::transport(operation))
}
