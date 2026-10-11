//! The binary owns execution and output. Parsing help/version never opens an engine.
use darktable_edit::{
    command::{OutputMode, parse_from},
    protocol::{ToolError, success_envelope},
};
use serde_json::Value;
use std::io::{self, Write};

fn write_result(value: &Value, mode: OutputMode) -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if mode == OutputMode::Json {
        serde_json::to_writer(&mut stdout, value)?;
    } else {
        serde_json::to_writer_pretty(&mut stdout, value)?;
    }
    stdout.write_all(b"\n")?;
    stdout.flush()
}
fn main() -> std::process::ExitCode {
    let argv: Vec<_> = std::env::args_os().collect();
    let error_output = if argv
        .windows(2)
        .any(|pair| pair[0] == "--output" && pair[1] == "text")
        || argv.iter().any(|arg| arg == "--output=text")
    {
        OutputMode::Text
    } else {
        OutputMode::Json
    };
    let parsed = match parse_from(argv) {
        Ok(parsed) => parsed,
        Err(error) if error.exit_code == 0 => {
            if let Err(failure) = io::stdout().write_all(error.message.as_bytes()) {
                eprintln!("Cannot write help/version: {failure}");
                return 6.into();
            }
            return 0.into();
        }
        Err(error) => {
            if let Err(failure) = write_result(&error.envelope(), error_output) {
                eprintln!("Cannot report {}: {failure}", error.code);
                return 6.into();
            }
            return error.exit_code.into();
        }
    };
    let result = darktable_edit::execute(&parsed.invocation);
    let (envelope, exit_code) = match result {
        Ok(data) => (success_envelope(data), 0),
        Err(error) => (error.envelope(), error.exit_code),
    };
    if let Err(failure) = write_result(&envelope, parsed.output) {
        // Stdout may already contain a prefix: never append a second machine envelope.
        let data = &envelope["data"];
        let committed = data.get("fileCommitted").and_then(Value::as_bool) == Some(true)
            || envelope["error"]["details"]
                .get("fileCommitted")
                .and_then(Value::as_bool)
                == Some(true);
        let effect = if committed
            || envelope["status"] == "ok" && is_mutation(&parsed.invocation.operation)
        {
            "partial"
        } else if envelope["error"]["effect"] == "unknown" {
            "unknown"
        } else {
            "none"
        };
        let error = ToolError::new(
            "local_io_failed",
            format!("Cannot report command result: {failure}"),
            6,
        )
        .effect(effect);
        eprintln!(
            "{}; effect={}; fileCommitted={}; confirmedResult={}",
            error, error.effect, committed, envelope
        );
        return if effect == "unknown" {
            7.into()
        } else {
            6.into()
        };
    }
    exit_code.into()
}
fn is_mutation(operation: &str) -> bool {
    matches!(
        operation,
        "open"
            | "edit set"
            | "edit apply"
            | "edit reset"
            | "edit rebind"
            | "edit migrate"
            | "edit instances add"
            | "edit instances clone"
            | "edit instances enable"
            | "edit instances remove"
            | "edit instances reorder"
            | "edit masks create"
            | "edit masks set"
            | "edit masks remove"
            | "edit actions run"
            | "edit output set"
            | "presets save"
            | "presets apply"
            | "presets import"
            | "presets remove"
            | "styles save"
            | "styles apply"
            | "styles import"
            | "styles remove"
            | "resources import"
            | "resources remove"
            | "history restore"
    )
}
