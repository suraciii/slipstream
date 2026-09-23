use clap::{Parser, error::ErrorKind};
use slipstream_cli::{
    Cli, InvocationResult, invalid_invocation, invoke_until, parse_error_preferences,
};
use std::{
    env,
    io::{self, Write},
    process::ExitCode,
    sync::mpsc,
    time::Duration,
};

async fn publish_until(result: InvocationResult, deadline: tokio::time::Instant) -> u8 {
    let exit_code = result.exit_code;
    let committed_path = result.committed_preview_path.clone();
    let (result_send, result_receive) = mpsc::sync_channel(1);
    let (completion_send, mut completion_receive) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let write_result = result_receive
            .recv()
            .map_err(|_| io::Error::other("command result publisher stopped"))
            .and_then(|result: InvocationResult| {
                let mut stdout = io::stdout().lock();
                stdout.write_all(result.stdout.as_bytes())?;
                stdout.flush()
            });
        let _ = completion_send.send(write_result);
    });
    result_send
        .send(result)
        .expect("result publisher is waiting for one command result");

    // A network timeout can settle exactly at the absolute deadline. Give an
    // already-waiting writer a scheduling opportunity so writable stdout still
    // receives the timeout envelope, without waiting on blocked output.
    if deadline <= tokio::time::Instant::now() || exit_code == 130 {
        let scheduling_settlement = std::time::Instant::now() + Duration::from_millis(25);
        while std::time::Instant::now() < scheduling_settlement {
            match completion_receive.try_recv() {
                Ok(result) => {
                    return if result.is_ok() {
                        exit_code
                    } else {
                        publication_failure(
                            committed_path.as_deref(),
                            if exit_code == 130 { 130 } else { 6 },
                        )
                    };
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::yield_now();
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    return publication_failure(
                        committed_path.as_deref(),
                        if exit_code == 130 { 130 } else { 6 },
                    );
                }
            }
        }
        std::process::exit(i32::from(publication_failure(
            committed_path.as_deref(),
            if exit_code == 130 { 130 } else { 6 },
        )));
    }

    tokio::select! {
        completion = &mut completion_receive => {
            if completion.is_ok_and(|result| result.is_ok()) {
                exit_code
            } else {
                publication_failure(
                    committed_path.as_deref(),
                    if exit_code == 130 { 130 } else { 6 },
                )
            }
        }
        _ = tokio::time::sleep_until(deadline) => {
            std::process::exit(i32::from(publication_failure(committed_path.as_deref(), 6)));
        }
        _ = tokio::signal::ctrl_c() => {
            std::process::exit(i32::from(publication_failure(committed_path.as_deref(), 130)));
        }
    }
}

fn publication_failure(committed_path: Option<&str>, exit_code: u8) -> u8 {
    if let Some(path) = committed_path {
        let escaped = serde_json::to_string(path).expect("path serialization is infallible");
        let message = format!(
            "Preview file was already published at {escaped}; inspect it before retrying.\n"
        );
        // The stdout deadline must not turn into an unbounded stderr write.
        unsafe {
            let flags = libc::fcntl(libc::STDERR_FILENO, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(libc::STDERR_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK);
                libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len());
            }
        }
    }
    exit_code
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = env::args_os().collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let exit_code = error.exit_code();
            if error.print().is_err() {
                return ExitCode::from(6);
            }
            return ExitCode::from(exit_code as u8);
        }
        Err(error) => {
            let preferences = parse_error_preferences(&arguments);
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(preferences.timeout_seconds);
            return ExitCode::from(
                publish_until(
                    invalid_invocation(preferences.output, error.to_string()),
                    deadline,
                )
                .await,
            );
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.timeout);
    let server_environment = match env::var("SLIPSTREAM_SERVER_URL") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => Some(String::new()),
    };
    let result = invoke_until(cli, server_environment.as_deref(), deadline).await;
    let exit_code = publish_until(result, deadline).await;
    // Terminal exit bounds executable teardown. Returning through the async
    // runtime's destructor would join a still-blocked input worker held open
    // on `--input -`, outliving the whole-command deadline the published
    // envelope just reported.
    std::process::exit(i32::from(exit_code))
}
