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

async fn publish_until(result: InvocationResult, deadline: tokio::time::Instant) -> ExitCode {
    let exit_code = result.exit_code;
    let (result_send, result_receive) = mpsc::sync_channel(1);
    let (completion_send, mut completion_receive) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let write_result = result_receive
            .recv()
            .map_err(|_| io::Error::other("command result publisher stopped"))
            .and_then(|result: InvocationResult| {
                io::stdout().lock().write_all(result.stdout.as_bytes())
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
                Ok(result) => return ExitCode::from(if result.is_ok() { exit_code } else { 6 }),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::yield_now();
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    return ExitCode::from(6);
                }
            }
        }
        std::process::exit(if exit_code == 130 { 130 } else { 6 });
    }

    tokio::select! {
        completion = &mut completion_receive => {
            ExitCode::from(if completion.is_ok_and(|result| result.is_ok()) { exit_code } else { 6 })
        }
        _ = tokio::time::sleep_until(deadline) => {
            std::process::exit(if exit_code == 130 { 130 } else { 6 });
        }
        _ = tokio::signal::ctrl_c() => {
            std::process::exit(130);
        }
    }
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
            return publish_until(
                invalid_invocation(preferences.output, error.to_string()),
                deadline,
            )
            .await;
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.timeout);
    let server_environment = match env::var("SLIPSTREAM_SERVER_URL") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => Some(String::new()),
    };
    let result = invoke_until(cli, server_environment.as_deref(), deadline).await;
    publish_until(result, deadline).await
}
