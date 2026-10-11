use super::*;

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Create or reopen a Photo's real workspace directory.
    Open {
        #[arg(value_parser = nonempty)]
        photo_id: String,
    },
    /// Read directory, source evidence, edits and deletion inventory.
    Show {
        #[arg(value_parser = nonempty)]
        workspace_id: String,
    },
    /// List completed workspace files, or continue an opaque cursor.
    Files {
        #[arg(value_parser = nonempty, required_unless_present = "cursor", conflicts_with = "cursor")]
        workspace_id: Option<String>,
        #[arg(long, value_parser = ["input", "preview", "output", "resource", "file"], conflicts_with = "cursor")]
        kind: Option<String>,
        #[arg(long, value_parser = page_limit, conflicts_with = "cursor")]
        limit: Option<u8>,
        #[arg(long, value_parser = nonempty)]
        cursor: Option<String>,
    },
    /// Inspect exact file revision and verified byte facts.
    Inspect {
        workspace_id: String,
        artifact_id: String,
    },
    /// Download verified bytes to a new caller-local path.
    Download {
        workspace_id: String,
        artifact_id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        revision: Option<String>,
    },
    /// Remove an explicitly selected unreferenced file.
    Remove {
        workspace_id: String,
        artifact_id: String,
        #[arg(long)]
        revision: Option<String>,
        #[arg(long)]
        request: Option<String>,
    },
    /// Prepare a protected input from acknowledged Library source evidence.
    RefreshInput {
        workspace_id: String,
        #[arg(long)]
        input: String,
        #[arg(long)]
        request: Option<String>,
    },
    /// Delete the complete reviewed inventory with an explicit confirmation.
    Delete {
        workspace_id: String,
        #[arg(long)]
        if_state: String,
        #[arg(long)]
        request: String,
        #[arg(long, required = true)]
        confirm: bool,
    },
}

impl WorkspaceCommand {
    pub(crate) fn operation(&self) -> Operation {
        Operation::Workspace(match self {
            Self::Open { .. } => "workspace-open",
            Self::Show { .. } => "workspace-show",
            Self::Files { .. } => "workspace-files",
            Self::Inspect { .. } => "workspace-inspect",
            Self::Download { .. } => "workspace-download",
            Self::Remove { .. } => "workspace-remove",
            Self::RefreshInput { .. } => "workspace-refresh-input",
            Self::Delete { .. } => "workspace-delete",
        })
    }
}

pub(crate) struct Prepared {
    pub body: Option<Value>,
    pub destination: Option<preview_download::Destination>,
}
pub(crate) fn prepare(command: &WorkspaceCommand) -> Result<Prepared, CommandFailure> {
    let (workspace, artifact, revision) = match command {
        WorkspaceCommand::Open { photo_id } => (Some(photo_id.as_str()), None, None),
        WorkspaceCommand::Show { workspace_id }
        | WorkspaceCommand::RefreshInput { workspace_id, .. }
        | WorkspaceCommand::Delete { workspace_id, .. } => {
            (Some(workspace_id.as_str()), None, None)
        }
        WorkspaceCommand::Files { workspace_id, .. } => (workspace_id.as_deref(), None, None),
        WorkspaceCommand::Inspect {
            workspace_id,
            artifact_id,
        } => (
            Some(workspace_id.as_str()),
            Some(artifact_id.as_str()),
            None,
        ),
        WorkspaceCommand::Download {
            workspace_id,
            artifact_id,
            revision,
            ..
        }
        | WorkspaceCommand::Remove {
            workspace_id,
            artifact_id,
            revision,
            ..
        } => (
            Some(workspace_id.as_str()),
            Some(artifact_id.as_str()),
            revision.as_deref(),
        ),
    };
    for value in [workspace, artifact, revision].into_iter().flatten() {
        if !valid_request_identity(value) {
            return Err(CommandFailure::invalid(
                "identifier",
                "Invalid workspace, artifact or revision identifier.",
            ));
        }
    }
    if let WorkspaceCommand::Delete { if_state, .. } = command
        && (if_state.is_empty()
            || if_state.len() > 512
            || !if_state
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
    {
        return Err(CommandFailure::invalid(
            "ifState",
            "Invalid inventory token.",
        ));
    }
    let mut body = None;
    let mut destination = None;
    match command {
        WorkspaceCommand::Download { file, .. } => {
            destination = Some(preview_download::Destination::preflight(
                preview_download::DestinationKind::ProcessingArtifact,
                file,
            )?)
        }
        WorkspaceCommand::RefreshInput { input, request, .. } => {
            let value =
                darktable_edit::command::read_command_input(input, &mut std::io::stdin().lock())
                    .map_err(super::darktable::tool_failure)?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Refresh {
                expected_source_revision: String,
            }
            let document: Refresh = serde_json::from_value(value)
                .map_err(|error| CommandFailure::invalid("input", error.to_string()))?;
            if document.expected_source_revision.is_empty()
                || document.expected_source_revision.len() > MAXIMUM_SOURCE_REVISION_BYTES
            {
                return Err(CommandFailure::invalid(
                    "expectedSourceRevision",
                    "Expected nonempty source evidence of at most 16384 bytes.",
                ));
            }
            body = Some(
                json!({"expectedSourceRevision":document.expected_source_revision,"requestId":request}),
            );
        }
        WorkspaceCommand::Remove {
            revision, request, ..
        } => body = Some(json!({"revision":revision,"requestId":request})),
        WorkspaceCommand::Delete {
            if_state,
            request,
            confirm,
            ..
        } => body = Some(json!({"ifState":if_state,"requestId":request,"confirm":confirm})),
        WorkspaceCommand::Open { .. } => body = Some(json!({})),
        _ => {}
    }
    if let Some(body) = body.as_mut().and_then(Value::as_object_mut) {
        body.retain(|_, value| !value.is_null());
    }
    let request = match command {
        WorkspaceCommand::Remove { request, .. }
        | WorkspaceCommand::RefreshInput { request, .. } => request.as_deref(),
        WorkspaceCommand::Delete { request, .. } => Some(request.as_str()),
        _ => None,
    };
    if request.is_some_and(|id| !valid_request_identity(id)) {
        return Err(CommandFailure::invalid(
            "request",
            "Invalid request identity.",
        ));
    }
    Ok(Prepared { body, destination })
}

pub(crate) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    publication: &PublicationState,
    command: &WorkspaceCommand,
    prepared: Prepared,
) -> Result<Value, CommandFailure> {
    let operation = command.operation();
    let (method, url) = match command {
        WorkspaceCommand::Open { photo_id } => (
            Method::POST,
            client.endpoint(&["api", "photos", photo_id, "workspace"]),
        ),
        WorkspaceCommand::Show { workspace_id } => (
            Method::GET,
            client.endpoint(&["api", "workspaces", workspace_id]),
        ),
        WorkspaceCommand::Files {
            workspace_id,
            kind,
            limit,
            cursor,
        } => {
            let mut url = if let Some(id) = workspace_id {
                client.endpoint(&["api", "workspaces", id, "files"])
            } else {
                client.endpoint(&["api", "workspace-files"])
            };
            if let Some(cursor) = cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            } else {
                url.query_pairs_mut()
                    .append_pair("limit", &limit.unwrap_or(50).to_string());
                if let Some(kind) = kind {
                    url.query_pairs_mut().append_pair("kind", kind);
                }
            }
            (Method::GET, url)
        }
        WorkspaceCommand::Inspect {
            workspace_id,
            artifact_id,
        } => (
            Method::GET,
            client.endpoint(&["api", "workspaces", workspace_id, "files", artifact_id]),
        ),
        WorkspaceCommand::Download {
            workspace_id,
            artifact_id,
            revision,
            ..
        } => {
            let evidence: Value = client
                .json(
                    operation,
                    Method::GET,
                    client.endpoint(&["api", "workspaces", workspace_id, "files", artifact_id]),
                    None,
                )
                .await?;
            let observed = evidence["revision"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CommandFailure::transport(operation))?
                .to_owned();
            if revision
                .as_ref()
                .is_some_and(|expected| expected != &observed)
            {
                return Err(CommandFailure::from_payload(
                    4,
                    ErrorPayload {
                        code: "artifact_changed".into(),
                        message: "The selected file revision changed.".into(),
                        effect: "none".into(),
                        details: json!({"workspaceId":workspace_id,"artifactId":artifact_id,"expectedRevision":revision,"currentRevision":observed}),
                    },
                ));
            }
            let mut url = client.endpoint(&[
                "api",
                "workspaces",
                workspace_id,
                "files",
                artifact_id,
                "bytes",
            ]);
            url.query_pairs_mut().append_pair("revision", &observed);
            return super::darktable::download(
                client,
                operation,
                url,
                prepared.destination.expect("prepared download"),
                publication,
                Some(&evidence),
            )
            .await;
        }
        WorkspaceCommand::Remove {
            workspace_id,
            artifact_id,
            ..
        } => (
            Method::DELETE,
            client.endpoint(&["api", "workspaces", workspace_id, "files", artifact_id]),
        ),
        WorkspaceCommand::RefreshInput { workspace_id, .. } => (
            Method::POST,
            client.endpoint(&["api", "workspaces", workspace_id, "refresh-input"]),
        ),
        WorkspaceCommand::Delete { workspace_id, .. } => (
            Method::DELETE,
            client.endpoint(&["api", "workspaces", workspace_id]),
        ),
    };
    if method == Method::GET {
        return client.json(operation, method, url, None).await;
    }
    let identity = MutationIdentity {
        operation,
        photo_ids: match command {
            WorkspaceCommand::Open { photo_id } => vec![photo_id.clone()],
            _ => vec![],
        },
        album_id: None,
        album_name: None,
        mappings: vec![],
    };
    let body = prepared.body;
    let (_, bytes) = client
        .mutation_statuses(
            method,
            &identity,
            admission,
            url,
            body.clone(),
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await
        .map_err(|mut failure| {
            if failure.payload.effect == "unknown" {
                failure.payload.details["request"] = body.clone().unwrap_or(Value::Null);
            }
            failure
        })?;
    serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))
}
