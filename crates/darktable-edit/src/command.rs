//! Standalone grammar and normalization; this module has no engine dependency.
mod validation;

use crate::protocol::{Invocation, ToolError};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value};
use std::{ffi::OsString, io::Read, path::PathBuf};

pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Json,
    Text,
}
#[derive(Clone, Debug)]
pub struct ParsedCommand {
    pub invocation: Invocation,
    pub output: OutputMode,
}

fn option(name: &'static str) -> Arg {
    Arg::new(name).long(name).num_args(1)
}
fn positional(name: &'static str) -> Arg {
    Arg::new(name).required(true)
}
fn target(command: Command) -> Command {
    command.arg(positional("target"))
}
fn scope(command: Command) -> Command {
    command.arg(option("state"))
}
fn guard(command: Command, required: bool) -> Command {
    command
        .arg(option("if-state").required(required))
        .arg(option("request"))
}
fn observed(command: Command) -> Command {
    command.arg(option("if-state"))
}
fn input(command: Command) -> Command {
    command.arg(option("input").required(true))
}
fn catalog(command: Command) -> Command {
    command.arg(option("catalog").value_parser(["local", "global"]))
}
fn version(command: Command) -> Command {
    command.arg(option("version").required(true))
}
fn page(mut command: Command) -> Command {
    // A continuation owns its target/filter identity. Even --limit cannot alter it.
    let conflicts: Vec<_> = command
        .get_arguments()
        .map(|argument| argument.get_id().clone())
        .collect();
    command = command.mut_args(|argument| {
        if argument.get_id() == "target" {
            argument.required(false).required_unless_present("cursor")
        } else {
            argument
        }
    });
    command
        .arg(
            option("limit")
                .value_parser(clap::value_parser!(u64).range(1..=60))
                .conflicts_with("cursor"),
        )
        .arg(option("cursor").conflicts_with_all(conflicts))
}
fn leaf(name: &'static str) -> Command {
    Command::new(name)
}

pub fn grammar() -> Command {
    let instances = leaf("instances")
        .subcommand_required(true)
        .subcommand(page(observed(target(leaf("list")))))
        .subcommand(observed(target(leaf("show")).arg(positional("instance"))))
        .subcommand(guard(
            target(leaf("add"))
                .arg(positional("module"))
                .arg(option("name")),
            false,
        ))
        .subcommand(guard(
            target(leaf("clone"))
                .arg(positional("instance"))
                .arg(option("name")),
            false,
        ))
        .subcommand(guard(
            target(leaf("enable")).arg(positional("instance")).arg(
                option("value")
                    .required(true)
                    .value_parser(["true", "false"]),
            ),
            false,
        ))
        .subcommand(guard(
            target(leaf("remove")).arg(positional("instance")),
            false,
        ))
        .subcommand(guard(input(target(leaf("reorder"))), true));
    let masks = leaf("masks")
        .subcommand_required(true)
        .subcommand(page(observed(target(leaf("list")))))
        .subcommand(observed(target(leaf("show")).arg(positional("mask"))))
        .subcommand(guard(input(target(leaf("create"))), false))
        .subcommand(guard(
            target(leaf("set"))
                .arg(positional("mask"))
                .arg(option("value").required(true)),
            false,
        ))
        .subcommand(guard(target(leaf("remove")).arg(positional("mask")), false));
    let actions = leaf("actions")
        .subcommand_required(true)
        .subcommand(page(observed(target(leaf("list")).arg(option("instance")))))
        .subcommand(observed(
            target(leaf("describe"))
                .arg(positional("action"))
                .arg(option("instance")),
        ))
        .subcommand(guard(
            input(
                target(leaf("run"))
                    .arg(positional("action"))
                    .arg(option("instance")),
            ),
            false,
        ));
    let edit = leaf("edit")
        .subcommand_required(true)
        .subcommand(target(leaf("show")))
        .subcommand(guard(
            target(leaf("set"))
                .arg(positional("instance"))
                .arg(positional("parameter"))
                .arg(option("value").required(true)),
            false,
        ))
        .subcommand(observed(catalog(input(target(leaf("validate"))))))
        .subcommand(guard(catalog(input(target(leaf("apply")))), true))
        .subcommand(guard(
            target(leaf("reset"))
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["instance", "parameter"])
                        .required_unless_present("instance"),
                )
                .arg(option("instance").conflicts_with("all"))
                .arg(option("parameter").requires("instance")),
            false,
        ))
        .subcommand(guard(input(target(leaf("rebind"))), true))
        .subcommand(instances)
        .subcommand(masks)
        .subcommand(actions)
        .subcommand(
            leaf("output")
                .subcommand_required(true)
                .subcommand(observed(target(leaf("show"))))
                .subcommand(guard(
                    target(leaf("set"))
                        .arg(positional("field"))
                        .arg(option("value").required(true)),
                    false,
                )),
        )
        .subcommand(
            leaf("migration").subcommand_required(true).subcommand(
                target(leaf("show"))
                    .arg(option("bundle").required(true))
                    .arg(option("schema").required(true)),
            ),
        )
        .subcommand(
            guard(
                target(leaf("migrate"))
                    .arg(option("migration").required(true))
                    .arg(
                        option("confirm")
                            .num_args(0)
                            .action(ArgAction::SetTrue)
                            .required(true),
                    ),
                true,
            )
            .mut_arg("request", |arg| arg.required(true)),
        );
    let modules = leaf("modules")
        .subcommand_required(true)
        .subcommand(page(observed(scope(leaf("list")).arg(option("search")))))
        .subcommand(observed(scope(leaf("describe")).arg(positional("module"))));
    let resources = leaf("resources")
        .subcommand_required(true)
        .subcommand(scope(leaf("kinds")))
        .subcommand(page(scope(leaf("list")).arg(option("kind"))))
        .subcommand(version(scope(leaf("show")).arg(positional("resource"))))
        .subcommand(
            scope(leaf("import"))
                .arg(option("kind").required(true))
                .arg(option("file").required(true))
                .arg(option("request")),
        )
        .subcommand(
            version(scope(leaf("remove")).arg(positional("resource"))).arg(option("request")),
        );
    let presets = definitions("presets", "preset")
        .mut_subcommand("list", |cmd| page(scope(cmd).arg(option("module"))))
        .subcommand(guard(
            catalog(input(target(leaf("save")).arg(positional("instance")))),
            false,
        ))
        .subcommand(guard(
            catalog(
                version(target(leaf("apply")).arg(positional("preset")))
                    .arg(option("instance").required(true))
                    .arg(
                        option("mode")
                            .required(true)
                            .value_parser(["update", "replace"]),
                    )
                    .arg(
                        option("initialization")
                            .required(true)
                            .value_parser(["literal", "target-camera"]),
                    )
                    .arg(
                        option("mask-policy")
                            .required(true)
                            .value_parser(["omit", "compatible"]),
                    ),
            ),
            true,
        ));
    let styles = definitions("styles", "style")
        .mut_subcommand("list", |cmd| page(scope(cmd)))
        .subcommand(guard(catalog(input(target(leaf("save")))), false))
        .subcommand(guard(
            catalog(input(version(
                target(leaf("apply")).arg(positional("style")),
            ))),
            true,
        ));
    Command::new("darktable-edit")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Independent semantic darktable editing of local files")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(option("output").global(true).value_parser(["json", "text"]))
        .subcommand(leaf("__invoke").hide(true))
        .subcommand(target(leaf("__inspect").hide(true)))
        .subcommand(
            leaf("open")
                .arg(positional("input-path"))
                .arg(option("state").required(true))
                .arg(option("preview-directory"))
                .arg(option("output-directory")),
        )
        .subcommand(modules)
        .subcommand(edit)
        .subcommand(resources)
        .subcommand(presets)
        .subcommand(styles)
        .subcommand(
            leaf("preview")
                .subcommand_required(true)
                .subcommand(render("render").arg(option("input"))),
        )
        .subcommand(
            leaf("export")
                .subcommand_required(true)
                .subcommand(render("create"))
                .subcommand(
                    target(leaf("state-file"))
                        .arg(option("revision"))
                        .arg(option("file").required(true))
                        .arg(option("request")),
                ),
        )
        .subcommand(
            leaf("history")
                .subcommand_required(true)
                .subcommand(page(target(leaf("list"))))
                .subcommand(target(leaf("show")).arg(option("revision").required(true)))
                .subcommand(guard(
                    target(leaf("restore")).arg(option("revision").required(true)),
                    true,
                )),
        )
}
fn definitions(name: &'static str, id: &'static str) -> Command {
    leaf(name)
        .subcommand_required(true)
        .subcommand(leaf("list"))
        .subcommand(version(scope(leaf("show")).arg(positional(id))))
        .subcommand(
            scope(leaf("import"))
                .arg(option("file").required(true))
                .arg(option("request")),
        )
        .subcommand(
            version(scope(leaf("export")).arg(positional(id))).arg(option("file").required(true)),
        )
        .subcommand(version(scope(leaf("remove")).arg(positional(id))).arg(option("request")))
}
fn render(name: &'static str) -> Command {
    target(leaf(name))
        .arg(option("revision"))
        .arg(option("directory"))
        .arg(option("name"))
        .arg(option("request"))
}

/// Parse argv including executable name. Help/version return a zero-exit display
/// error so callers can print the supplied text without starting the engine.
pub fn parse_from<I, T>(arguments: I) -> Result<ParsedCommand, ToolError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    parse_from_with_reader(arguments, &mut std::io::stdin().lock())
}
/// Reader injection permits bounded stdin parsing in tests and embedding tools.
pub fn parse_from_with_reader<I, T>(
    arguments: I,
    stdin: &mut dyn Read,
) -> Result<ParsedCommand, ToolError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = grammar().try_get_matches_from(arguments).map_err(|error| {
        let display = matches!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        );
        ToolError::new(
            if display { "display" } else { "invalid_input" },
            error.to_string(),
            if display { 0 } else { 2 },
        )
    })?;
    let output = if matches.get_one::<String>("output").map(String::as_str) == Some("text") {
        OutputMode::Text
    } else {
        OutputMode::Json
    };
    let mut current = &matches;
    let mut words = Vec::new();
    while let Some((name, subcommand)) = current.subcommand() {
        words.push(name);
        current = subcommand;
    }
    let operation = words.join(" ");
    if operation == "__invoke" {
        let value = photo_files::read_json(stdin, MAX_COMMAND_BYTES).map_err(json_error)?;
        let invocation: Invocation = serde_json::from_value(value)
            .map_err(|error| ToolError::new("invalid_input", error.to_string(), 2))?;
        validate_invocation(&invocation)?;
        return Ok(ParsedCommand { invocation, output });
    }
    normalize(operation, current, stdin).map(|invocation| ParsedCommand { invocation, output })
}
fn normalize(
    operation: String,
    matches: &ArgMatches,
    stdin: &mut dyn Read,
) -> Result<Invocation, ToolError> {
    let mut arguments = Map::new();
    let mut state_directory = None;
    for id in matches.ids() {
        let name = id.as_str();
        if name == "output"
            || name == "help"
            || name == "version" && matches.try_get_one::<String>(name).ok().flatten().is_none()
        {
            continue;
        }
        if name == "all" || name == "confirm" {
            if matches.get_flag(name) {
                arguments.insert(name.into(), true.into());
            }
            continue;
        }
        if name == "limit" {
            if let Some(value) = matches.get_one::<u64>(name) {
                arguments.insert(name.into(), (*value).into());
            }
            continue;
        }
        let Some(value) = matches.try_get_one::<String>(name).ok().flatten() else {
            continue;
        };
        if name == "target" || name == "state" {
            state_directory = Some(PathBuf::from(value));
            continue;
        }
        let key = argument_key(name);
        let parsed = if name == "value" {
            photo_files::read_json(value.as_bytes(), MAX_COMMAND_BYTES).map_err(json_error)?
        } else if name == "input" {
            read_input(value, stdin)?
        } else {
            Value::String(value.clone())
        };
        arguments.insert(key.into(), parsed);
    }
    validation::validate_arguments(&operation, &arguments)?;
    Ok(Invocation {
        operation,
        state_directory,
        arguments: Value::Object(arguments),
    })
}
fn json_error(error: std::io::Error) -> ToolError {
    ToolError::new("invalid_input", format!("Invalid command JSON: {error}"), 2)
}
fn read_input(path: &str, stdin: &mut dyn Read) -> Result<Value, ToolError> {
    if path == "-" {
        return photo_files::read_json(stdin, MAX_COMMAND_BYTES).map_err(json_error);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| {
        ToolError::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                "file_not_found"
            } else {
                "local_io_failed"
            },
            format!("Cannot read command input {path:?}: {error}"),
            if error.kind() == std::io::ErrorKind::NotFound {
                3
            } else {
                6
            },
        )
        .detail(serde_json::json!({"operation":"read-input","path":path,"fileCommitted":false}))
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| ToolError::new("local_io_failed", error.to_string(), 6))?;
    if !metadata.is_file() {
        return Err(ToolError::new(
            "invalid_input",
            "Command input must be a regular JSON file",
            2,
        ));
    }
    photo_files::read_json(file, MAX_COMMAND_BYTES).map_err(json_error)
}

/// Validate the semantic carrier against the same grammar as public argv.
/// This admits no unknown operations, fields, missing guards, or JSON file paths.
pub fn validate_invocation(invocation: &Invocation) -> Result<(), ToolError> {
    if invocation.operation.starts_with("__") {
        return validate_private_invocation(invocation);
    }
    if matches!(
        invocation.operation.as_str(),
        "preview render" | "export create" | "export state-file"
    ) && invocation.arguments.get("captureId").is_some()
    {
        return validate_captured_execution(invocation);
    }
    let mut command = grammar();
    let words: Vec<_> = invocation.operation.split(' ').collect();
    if words.is_empty()
        || words.iter().any(|word| word.is_empty())
        || invocation.operation == "__invoke"
    {
        return Err(ToolError::new(
            "invalid_input",
            "Invalid invocation operation",
            2,
        ));
    }
    for word in &words {
        command = command
            .find_subcommand(word)
            .cloned()
            .ok_or_else(|| ToolError::new("invalid_input", "Unknown invocation operation", 2))?;
    }
    command.build();
    if command.get_subcommands().next().is_some() {
        return Err(ToolError::new(
            "invalid_input",
            "Incomplete invocation operation",
            2,
        ));
    }
    let arguments = invocation.arguments.as_object().ok_or_else(|| {
        ToolError::new("invalid_input", "Invocation arguments must be an object", 2)
    })?;
    let mut argv: Vec<OsString> = vec!["darktable-edit".into()];
    argv.extend(words.iter().map(|word| OsString::from(*word)));
    let mut remaining = arguments.clone();
    let mut input_bytes = Vec::new();
    let mut positional: Vec<_> = command
        .get_arguments()
        .filter(|arg| arg.get_index().is_some())
        .collect();
    positional.sort_by_key(|arg| arg.get_index());
    for argument in positional {
        let name = argument.get_id().as_str();
        if name == "target" {
            if let Some(path) = &invocation.state_directory
                && !arguments.contains_key("cursor")
            {
                argv.push(path.as_os_str().to_owned());
            }
        } else {
            let key = argument_key(name);
            if let Some(value) = remaining.remove(key) {
                argv.push(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            ToolError::new("invalid_input", format!("{key} must be a string"), 2)
                        })?
                        .into(),
                );
            }
        }
    }
    if !command.get_arguments().any(|arg| arg.get_id() == "target")
        && let Some(path) = &invocation.state_directory
        && !arguments.contains_key("cursor")
    {
        argv.push("--state".into());
        argv.push(path.as_os_str().to_owned());
    }
    for (key, value) in remaining {
        let argument = command
            .get_arguments()
            .find(|arg| argument_key(arg.get_id().as_str()) == key && arg.get_long().is_some())
            .ok_or_else(|| {
                ToolError::new(
                    "invalid_input",
                    format!("Unknown invocation argument: {key}"),
                    2,
                )
            })?;
        let name = argument.get_long().unwrap();
        if name == "output" || name == "help" {
            return Err(ToolError::new(
                "invalid_input",
                "Display options are not invocation arguments",
                2,
            ));
        }
        argv.push(format!("--{name}").into());
        if name == "all" || name == "confirm" {
            if value != Value::Bool(true) {
                return Err(ToolError::new(
                    "invalid_input",
                    format!("{key} must be true"),
                    2,
                ));
            }
        } else if name == "input" {
            argv.push("-".into());
            input_bytes = serde_json::to_vec(&value)
                .map_err(|error| ToolError::new("invalid_input", error.to_string(), 2))?;
        } else if name == "value" {
            argv.push(value.to_string().into());
        } else if name == "limit" {
            argv.push(
                value
                    .as_u64()
                    .ok_or_else(|| ToolError::new("invalid_input", "limit must be an integer", 2))?
                    .to_string()
                    .into(),
            );
        } else {
            argv.push(
                value
                    .as_str()
                    .ok_or_else(|| {
                        ToolError::new("invalid_input", format!("{key} must be a string"), 2)
                    })?
                    .into(),
            );
        }
    }
    parse_from_with_reader(argv, &mut input_bytes.as_slice()).map(|_| ())
}
fn argument_key(name: &str) -> &str {
    match name {
        "input-path" => "inputPath",
        "preview-directory" => "previewDirectory",
        "output-directory" => "outputDirectory",
        "instance" => "instanceId",
        "module" => "moduleId",
        "parameter" => "parameterId",
        "mask" => "maskId",
        "action" => "actionId",
        "field" => "fieldId",
        "resource" => "resourceId",
        "preset" => "presetId",
        "style" => "styleId",
        "if-state" => "ifState",
        "request" => "requestId",
        "mask-policy" => "maskPolicy",
        "bundle" => "bundleId",
        "schema" => "schemaVersion",
        "migration" => "migrationToken",
        other => other,
    }
}

#[cfg(test)]
mod tests;

/// Parse an integration suffix with Photo/Edit selectors and TARGET removed.
/// The service replaces the supplied placeholder with its authorized directory.
pub fn parse_suffix_with_reader<I, T>(
    suffix: I,
    state_directory: Option<PathBuf>,
    stdin: &mut dyn Read,
) -> Result<ParsedCommand, ToolError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let suffix: Vec<OsString> = suffix.into_iter().map(Into::into).collect();
    let mut command = grammar();
    let mut depth = 0;
    for word in &suffix {
        let Some(next) = word
            .to_str()
            .and_then(|word| command.find_subcommand(word))
            .cloned()
        else {
            break;
        };
        command = next;
        depth += 1;
    }
    if suffix.iter().any(|word| {
        word == "--state"
            || word
                .to_str()
                .is_some_and(|word| word.starts_with("--state="))
    }) {
        return Err(ToolError::new(
            "invalid_input",
            "Remote commands do not accept --state",
            2,
        ));
    }
    let has_target = command
        .get_arguments()
        .any(|argument| argument.get_id() == "target");
    let mut argv = vec![OsString::from("darktable-edit")];
    argv.extend(suffix[..depth].iter().cloned());
    if has_target {
        let path = state_directory.as_ref().ok_or_else(|| {
            ToolError::new("invalid_input", "This operation requires a target", 2)
        })?;
        argv.push(path.as_os_str().to_owned());
    }
    argv.extend(suffix[depth..].iter().cloned());
    if !has_target && let Some(path) = &state_directory {
        argv.push("--state".into());
        argv.push(path.as_os_str().to_owned());
    }
    parse_from_with_reader(argv, stdin)
}

/// Normalize an already parsed standalone grammar tree (including executable
/// root). Integration may supply a placeholder TARGET before calling this.
pub fn parse_matches(
    matches: &ArgMatches,
    stdin: &mut dyn Read,
) -> Result<ParsedCommand, ToolError> {
    let output = if matches
        .try_get_one::<String>("output")
        .ok()
        .flatten()
        .map(String::as_str)
        == Some("text")
    {
        OutputMode::Text
    } else {
        OutputMode::Json
    };
    let mut current = matches;
    let mut words = Vec::new();
    while let Some((name, child)) = current.subcommand() {
        words.push(name);
        current = child;
    }
    normalize(words.join(" "), current, stdin)
        .map(|invocation| ParsedCommand { invocation, output })
}

pub fn normalize_matches(
    operation: String,
    matches: &ArgMatches,
    stdin: &mut dyn Read,
) -> Result<Invocation, ToolError> {
    normalize(operation, matches, stdin)
}

pub fn read_command_input(path: &str, stdin: &mut dyn Read) -> Result<Value, ToolError> {
    read_input(path, stdin)
}

fn validate_private_invocation(invocation: &Invocation) -> Result<(), ToolError> {
    let (allowed, required, state_required): (&[&str], &[&str], bool) =
        match invocation.operation.as_str() {
            "__inspect" => (&[], &[], true),
            "__show-input" => (&["inputPath"], &["inputPath"], false),
            "__capture-render" => (
                &[
                    "operation",
                    "requestId",
                    "revision",
                    "directory",
                    "name",
                    "file",
                    "input",
                ],
                &["operation"],
                true,
            ),
            "__receipt" => (&["requestId"], &["requestId"], true),
            "__release-capture" => (&["captureId", "reason"], &["captureId", "reason"], true),
            "__retry-capture" => (
                &["captureId", "requestId"],
                &["captureId", "requestId"],
                true,
            ),
            "__migration-review" => (&["inputPath", "legacy"], &["inputPath", "legacy"], false),
            "__migration-import" => (
                &[
                    "inputPath",
                    "legacy",
                    "reviewed",
                    "confirm",
                    "requestId",
                    "ifState",
                    "previewDirectory",
                    "outputDirectory",
                ],
                &["inputPath", "legacy", "reviewed", "confirm"],
                true,
            ),
            "__restore-rebase" => (&["previousRoot", "restoredRoot", "roots"], &[], true),
            _ => {
                return Err(ToolError::new(
                    "invalid_input",
                    "Unknown private invocation operation",
                    2,
                ));
            }
        };
    if invocation.operation != "__show-input"
        && invocation.state_directory.is_some() != state_required
    {
        return Err(ToolError::new(
            "invalid_input",
            "Invalid private operation state directory",
            2,
        ));
    }
    let fields = private_fields(&invocation.arguments, allowed, required)?;
    if invocation.operation != "__capture-render" {
        validation::validate_arguments(&invocation.operation, fields)?;
    }
    match invocation.operation.as_str() {
        "__capture-render" => {
            let operation = fields["operation"].as_str().unwrap();
            if !matches!(
                operation,
                "preview render" | "export create" | "export state-file"
            ) {
                return Err(ToolError::new(
                    "invalid_input",
                    "Invalid capture render operation",
                    2,
                ));
            }
            let mut actual = invocation.clone();
            actual.operation = operation.into();
            actual
                .arguments
                .as_object_mut()
                .unwrap()
                .remove("operation");
            if let Some(comparison) = actual.arguments.pointer_mut("/input/comparison")
                && comparison.get("stateDirectory").is_some()
            {
                if operation != "preview render" {
                    return Err(ToolError::new(
                        "invalid_input",
                        "Comparisons require preview render",
                        2,
                    ));
                }
                let compared = private_fields(
                    comparison,
                    &["stateDirectory", "revision"],
                    &["stateDirectory"],
                )?;
                validation::validate_arguments("__comparison", compared)?;
                // Validate all public render fields without widening its comparison grammar.
                // The original carrier is preserved for runtime capture.
                *comparison = Value::String("baseline".into());
            }
            validate_invocation(&actual)
        }
        "__release-capture"
            if !matches!(fields["reason"].as_str(), Some("cancelled" | "failed")) =>
        {
            Err(ToolError::new(
                "invalid_input",
                "Invalid capture release reason",
                2,
            ))
        }
        "__migration-import" if fields["confirm"] != Value::Bool(true) => Err(ToolError::new(
            "invalid_input",
            "Reviewed migration requires confirm=true",
            2,
        )),
        "__restore-rebase" => {
            if let Some(roots) = fields.get("roots") {
                if fields.len() != 1 {
                    return Err(ToolError::new(
                        "invalid_input",
                        "roots cannot be mixed with root pair",
                        2,
                    ));
                }
                let roots = roots.as_array().unwrap();
                if roots.is_empty() {
                    return Err(ToolError::new("invalid_input", "roots must be nonempty", 2));
                }
                for root in roots {
                    private_fields(
                        root,
                        &["previousRoot", "restoredRoot"],
                        &["previousRoot", "restoredRoot"],
                    )?;
                }
            } else if !fields.contains_key("previousRoot") || !fields.contains_key("restoredRoot") {
                return Err(ToolError::new(
                    "invalid_input",
                    "Both root paths are required",
                    2,
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn private_fields<'a>(
    value: &'a Value,
    allowed: &[&str],
    required: &[&str],
) -> Result<&'a Map<String, Value>, ToolError> {
    let fields = value.as_object().ok_or_else(|| {
        ToolError::new("invalid_input", "Invocation arguments must be an object", 2)
    })?;
    for key in fields.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ToolError::new(
                "invalid_input",
                format!("Unknown private argument: {key}"),
                2,
            ));
        }
    }
    for key in required {
        if !fields.contains_key(*key) {
            return Err(ToolError::new(
                "invalid_input",
                format!("Missing private argument: {key}"),
                2,
            ));
        }
    }
    for (key, value) in fields {
        let valid = match key.as_str() {
            "legacy" | "reviewed" | "input" => value.is_object(),
            "roots" => value.is_array(),
            "confirm" => value.is_boolean(),
            _ => value.as_str().is_some_and(|text| !text.is_empty()),
        };
        if !valid {
            return Err(ToolError::new(
                "invalid_input",
                format!("Invalid private argument type: {key}"),
                2,
            ));
        }
    }
    Ok(fields)
}
fn validate_captured_execution(invocation: &Invocation) -> Result<(), ToolError> {
    if invocation.state_directory.is_none() {
        return Err(ToolError::new(
            "invalid_input",
            "Captured execution requires state directory",
            2,
        ));
    }
    let fields = private_fields(
        &invocation.arguments,
        &["captureId", "requestId"],
        &["captureId"],
    )?;
    validation::validate_arguments(&invocation.operation, fields)
}

/// Admit portable preset/style structure before remote upload; the tool checks
/// native parameter, blend, mask and output schemas against its qualified bundle.
pub fn validate_interchange(value: &Value, kind: &str) -> Result<(), ToolError> {
    if !matches!(kind, "preset" | "style")
        || value.get("kind").and_then(Value::as_str) != Some(kind)
    {
        return Err(ToolError::new(
            "invalid_input",
            "Interchange kind differs from command",
            2,
        ));
    }
    crate::catalog_interchange::validate(value)
        .map_err(|message| ToolError::new("invalid_input", message, 2))
}
