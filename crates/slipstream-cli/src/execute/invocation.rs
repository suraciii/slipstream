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
        }
    );
    let command = execute(&cli, environment, &admission, &publication);
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
            let failure = deadline_failure(operation, &publication, &admission);
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

pub(crate) fn render_text(envelope: &Envelope) -> String {
    match (&envelope.data, &envelope.error) {
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
