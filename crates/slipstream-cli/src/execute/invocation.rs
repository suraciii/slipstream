use super::*;

pub(crate) fn deadline_failure(
    operation: Operation,
    publication: &PublicationState,
    admission: &AdmissionState,
) -> CommandFailure {
    if let Some(data) = publication.committed() {
        CommandFailure::published_file(data, false, publication.committed_noun())
    } else if let Some(identity) = admission.admitted() {
        CommandFailure::unknown(&identity)
    } else if matches!(operation, Operation::LibraryCheck) {
        CommandFailure::library_check_deadline()
    } else {
        CommandFailure::transport(operation)
    }
}

pub async fn invoke(cli: Cli, environment: Option<&str>) -> InvocationResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.timeout);
    invoke_until(cli, environment, deadline).await
}

pub async fn invoke_until(
    cli: Cli,
    environment: Option<&str>,
    deadline: tokio::time::Instant,
) -> InvocationResult {
    let output = cli.output;
    let operation = command_operation(&cli.command);
    let admission = AdmissionState::default();
    let publication = PublicationState::default();
    let artifact_download = matches!(
        &cli.command,
        Command::Processing {
            command: ProcessingCommand::ArtifactDownload { .. }
        } | Command::Workspace {
            command: workspace::WorkspaceCommand::Download { .. }
        }
    ) || matches!(&cli.command, Command::Darktable(command) if command.operation.ends_with("download") || command.operation.ends_with("wait") || command.operation.ends_with(" export"));
    let cleanup_receipt = match &cli.command {
        Command::Trash {
            command: TrashCommand::CleanupOperation { operation_id },
        } => Some(operation_id.clone()),
        _ => None,
    };
    let command = execute(&cli, environment, &admission, &publication, deadline);
    tokio::pin!(command);
    let deadline_signal = async {
        if artifact_download {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep_until(deadline).await;
    };
    tokio::pin!(deadline_signal);
    let (exit_code, envelope) = tokio::select! {
        result = &mut command => match result {
            Ok(data) => (0, Envelope::success(data)),
            Err(failure) => {
                let envelope = match failure.data {
                    Some(data) if failure.payload.effect == "partial" => {
                        Envelope::partial(*data, failure.payload)
                    }
                    Some(data) => Envelope::error_with_data(*data, failure.payload),
                    None => Envelope::error(failure.payload),
                };
                (failure.exit_code, envelope)
            }
        },
        _ = &mut deadline_signal => {
            let failure = match cleanup_receipt.as_deref() {
                Some(operation_id) => crate::trash::unknown_receipt(operation_id),
                None => deadline_failure(operation, &publication, &admission),
            };
            let envelope = match failure.data {
                Some(data) => Envelope::partial(*data, failure.payload),
                None => Envelope::error(failure.payload),
            };
            (failure.exit_code, envelope)
        },
        _ = tokio::signal::ctrl_c() => {
            if let Some(data) = publication.committed() {
                let failure =
                    CommandFailure::published_file(data, true, publication.committed_noun());
                (130, Envelope::partial(*failure.data.unwrap(), failure.payload))
            } else {
                let failure = match admission.admitted() {
                Some(identity) => CommandFailure::interrupted_unknown(&identity),
                None => {
                    let mut failure = CommandFailure::transport(operation);
                    failure.payload.message = "The command was interrupted. Inspect status before continuing.".to_owned();
                    failure
                }
            };
            // A handled interruption exits 130 whether or not a request may
            // have been admitted; only the envelope distinguishes the cases.
            (130, Envelope::error(failure.payload))
            }
        }
    };
    render_invocation(
        output,
        exit_code,
        &envelope,
        publication
            .committed()
            .and_then(|value| value["path"].as_str().map(str::to_owned)),
    )
}

pub fn invalid_invocation(output: OutputFormat, reason: impl Into<String>) -> InvocationResult {
    let failure = CommandFailure::invalid("arguments", reason);
    render_invocation(
        output,
        failure.exit_code,
        &Envelope::error(failure.payload),
        None,
    )
}

pub(crate) fn render_invocation(
    output: OutputFormat,
    exit_code: u8,
    envelope: &Envelope,
    committed_preview_path: Option<String>,
) -> InvocationResult {
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string(envelope).expect("envelope serialization is infallible")
        ),
        OutputFormat::Text => render_text(envelope),
    };
    InvocationResult {
        exit_code,
        stdout,
        committed_preview_path,
    }
}

fn render_cleanup_text(data: &Value, error: Option<&ErrorPayload>) -> Option<String> {
    let counts = data.get("counts")?.as_object()?;
    let reviewed = data.get("reviewed")?.as_u64()?;
    if reviewed == 0 {
        return Some("Trash is empty.\n".to_owned());
    }
    let mut text = String::new();
    if let Some(error) = error {
        text.push_str(&format!("{}: {}\n", error.code, error.message));
    } else {
        text.push_str("Cleanup complete.\n");
    }
    if let Some(operation) = data.get("operationId").and_then(Value::as_str) {
        text.push_str(&format!("Operation: {}\n", operation.escape_debug()));
    }
    let count = |name: &str| counts.get(name).and_then(Value::as_u64).unwrap_or(0);
    text.push_str(&format!(
        "Cleaned {} · Present {} · Blocked {} · Changed {} · Unavailable {} · Inspection failed {}\n",
        count("cleaned"),
        count("present"),
        count("blocked"),
        count("changed"),
        count("unavailable"),
        count("inspectionFailed"),
    ));
    if let Some(items) = data.get("items").and_then(Value::as_array) {
        for item in items {
            let state = item
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let location = item
                .get("originalLocation")
                .and_then(Value::as_str)
                .unwrap_or("");
            let message = item.get("message").and_then(Value::as_str).unwrap_or("");
            let blockers = item
                .get("blockers")
                .and_then(Value::as_array)
                .map(|blockers| {
                    blockers
                        .iter()
                        .filter_map(|blocker| {
                            Some(format!(
                                "{}:{}",
                                blocker.get("kind")?.as_str()?,
                                blocker.get("name")?.as_str()?
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            text.push_str(&format!(
                "{} {}{}{}\n",
                state.escape_debug(),
                serde_json::to_string(location).expect("location serialization is infallible"),
                if message.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", message.escape_debug())
                },
                if blockers.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", blockers.join(", "))
                },
            ));
        }
    }
    Some(text)
}

pub(crate) fn render_text(envelope: &Envelope) -> String {
    if let Some(data) = &envelope.data
        && let Some(text) = render_cleanup_text(data, envelope.error.as_ref())
    {
        return text;
    }
    match (&envelope.data, &envelope.error) {
        (Some(data), None)
            if data.get("namespace").and_then(Value::as_str) == Some("service")
                && data.get("libraryRoot").and_then(Value::as_str).is_some()
                && data.get("photos").and_then(Value::as_array).is_some() =>
        {
            let root = data["libraryRoot"].as_str().unwrap();
            let mut text = format!("Success\nLibrary Folder: {}\n", root.escape_debug());
            for item in data["photos"].as_array().unwrap() {
                let kind = item
                    .get("originalKind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let marker = item
                    .get("removedAtMs")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "null".to_owned());
                let location = item
                    .get("originalLocation")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = item.get("photoId").and_then(Value::as_str).unwrap_or("");
                let original = item.get("originalId").and_then(Value::as_str).unwrap_or("");
                let available = item
                    .get("originalAvailable")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "null".to_owned());
                let state = item
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                text.push_str(&format!(
                    "{} {} marker={} location={} original={} available={} state={}\n",
                    id.escape_debug(),
                    kind.escape_debug(),
                    marker,
                    serde_json::to_string(location).expect("location serialization is infallible"),
                    original.escape_debug(),
                    available,
                    state.escape_debug()
                ));
            }
            let mut summary = data.clone();
            summary
                .as_object_mut()
                .expect("Trash listing is an object")
                .remove("photos");
            text.push_str(
                &serde_json::to_string_pretty(&summary)
                    .expect("result serialization is infallible"),
            );
            text.push('\n');
            text
        }
        (Some(data), None) => format!(
            "Success\n{}\n",
            serde_json::to_string_pretty(data).expect("result serialization is infallible")
        ),
        (_, Some(error)) => format!(
            "Error: {}\n{}\n{}\n",
            error.code,
            error.message,
            serde_json::to_string_pretty(&error.details)
                .expect("error serialization is infallible")
        ),
        _ => "Error: invalid result\n".to_owned(),
    }
}
