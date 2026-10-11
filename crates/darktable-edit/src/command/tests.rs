use super::*;
use serde_json::json;

fn parse(args: &[&str], input: &str) -> Result<ParsedCommand, ToolError> {
    parse_from_with_reader(
        std::iter::once("darktable-edit").chain(args.iter().copied()),
        &mut input.as_bytes(),
    )
}
#[test]
fn standalone_forms_cover_each_operation() {
    let forms: &[(&str, &str)] = &[
        (
            "open /input.raw --state /state --preview-directory /previews --output-directory /outputs",
            "",
        ),
        (
            "modules list --state /state --if-state token --search exposure --limit 60",
            "",
        ),
        ("modules describe exposure --state /state", ""),
        ("edit show /state", ""),
        ("edit instances list /state", ""),
        ("edit instances show /state instance", ""),
        ("edit masks list /state", ""),
        ("edit masks show /state mask", ""),
        ("edit actions list /state --instance instance", ""),
        ("edit actions describe /state action", ""),
        ("edit output show /state", ""),
        ("edit set /state instance parameter --value 0.5", ""),
        (
            "edit validate /state --input - --catalog global",
            r#"{"changes":[{"type":"set","instanceId":"instance","parameterId":"parameter","value":1}]}"#,
        ),
        (
            "edit apply /state --input - --if-state token",
            r#"{"changes":[{"type":"instance-enable","instanceId":"instance","enabled":true}]}"#,
        ),
        ("edit reset /state --all", ""),
        (
            "edit reset /state --instance instance --parameter parameter",
            "",
        ),
        (
            "edit rebind /state --input - --if-state token",
            r#"{"inputPath":"/new.raw"}"#,
        ),
        ("edit instances add /state module --name named", ""),
        ("edit instances clone /state instance", ""),
        ("edit instances enable /state instance --value true", ""),
        ("edit instances remove /state instance", ""),
        (
            "edit instances reorder /state --input - --if-state token",
            r#"{"instanceIds":["instance"]}"#,
        ),
        (
            "edit masks create /state --input -",
            r#"{"kind":"drawn","frameId":"frame","value":{}}"#,
        ),
        ("edit masks set /state mask --value {}", ""),
        ("edit masks remove /state mask", ""),
        ("edit actions run /state action --input -", "{}"),
        ("edit output set /state format --value 1", ""),
        ("resources kinds", ""),
        ("resources list --state /state --kind icc", ""),
        ("resources show resource --version version", ""),
        ("resources import --kind icc --file /large.icc", ""),
        (
            "resources remove resource --version version --request req",
            "",
        ),
        ("presets list --module module", ""),
        ("presets show preset --version version", ""),
        (
            "presets save /state instance --input -",
            r#"{"name":"Name","parameterIds":["parameter"],"includeBlend":false,"initialization":"literal","maskPolicy":"omit"}"#,
        ),
        (
            "presets apply /state preset --version version --instance instance --mode update --initialization literal --mask-policy omit --if-state token",
            "",
        ),
        ("presets import --file /preset.json", ""),
        (
            "presets export preset --version version --file /out.json",
            "",
        ),
        ("presets remove preset --version version", ""),
        ("styles list", ""),
        ("styles show style --version version", ""),
        (
            "styles save /state --input -",
            r#"{"name":"Name","instances":[{"instanceId":"instance","parameterIds":["parameter"],"includeBlend":false}],"includeOutput":false,"initialization":"literal","maskPolicy":"omit"}"#,
        ),
        (
            "styles apply /state style --version version --input - --if-state token",
            r#"{"mode":"merge","mapping":[{"entryId":"entry","instanceId":null}],"initialization":"literal","maskPolicy":"omit","includeOutput":false}"#,
        ),
        ("styles import --file /style.json", ""),
        ("styles export style --version version --file /out.json", ""),
        ("styles remove style --version version", ""),
        (
            "preview render /state --revision rev --directory /previews --name name --input - --request req",
            r#"{"comparison":{"revision":"rev"},"region":null}"#,
        ),
        (
            "export create /state --revision rev --directory /outputs --name name --request req",
            "",
        ),
        (
            "export state-file /state --revision rev --file /out.xmp --request req",
            "",
        ),
        ("history list /state --limit 60", ""),
        ("history show /state --revision rev", ""),
        ("history restore /state --revision rev --if-state token", ""),
        (
            "edit migration show /state --bundle bundle --schema schema",
            "",
        ),
        (
            "edit migrate /state --migration token --if-state token --request req --confirm",
            "",
        ),
    ];
    for (command, input) in forms {
        let args: Vec<_> = command.split_whitespace().collect();
        let parsed = parse(&args, input).unwrap_or_else(|error| panic!("{command}: {error}"));
        validate_invocation(&parsed.invocation)
            .unwrap_or_else(|error| panic!("carrier {command}: {error}"));
    }
}
#[test]
fn continuations_are_cursor_only() {
    for operation in [
        "modules list",
        "edit instances list",
        "edit masks list",
        "edit actions list",
        "resources list",
        "presets list",
        "styles list",
        "history list",
    ] {
        let mut args: Vec<_> = operation.split_whitespace().collect();
        args.extend(["--cursor", "token"]);
        let parsed = parse(&args, "").unwrap();
        assert_eq!(parsed.invocation.state_directory, None);
        assert_eq!(parsed.invocation.arguments, json!({"cursor":"token"}));
        let mut authorized = parsed.invocation.clone();
        authorized.state_directory = Some("/authorized/state".into());
        validate_invocation(&authorized).unwrap();
        args.extend(["--limit", "10"]);
        assert_eq!(parse(&args, "").unwrap_err().exit_code, 2);
    }
    assert!(parse(&["history", "list", "/state", "--cursor", "token"], "").is_err());
    assert!(
        parse(
            &[
                "resources",
                "list",
                "--cursor",
                "token",
                "--state",
                "/state"
            ],
            ""
        )
        .is_err()
    );
}
#[test]
fn strict_syntax_and_typed_json_precede_execution() {
    for args in [
        vec![
            "edit",
            "set",
            "/state",
            "instance",
            "parameter",
            "--value",
            "1",
            "--value",
            "2",
        ],
        vec![
            "edit",
            "instances",
            "enable",
            "/state",
            "instance",
            "--value",
            "1",
        ],
        vec![
            "edit",
            "reset",
            "/state",
            "--all",
            "--parameter",
            "parameter",
        ],
        vec!["modules", "list", "--photo", "photo"],
        vec!["modules", "list", "--limit", "61"],
        vec![
            "edit",
            "set",
            "/state",
            "bad/id",
            "parameter",
            "--value",
            "0",
        ],
    ] {
        assert_eq!(parse(&args, "").unwrap_err().exit_code, 2);
    }
    for input in [
        r#"{"changes":[],"unexpected":1}"#,
        r#"{"changes":[],"changes":[]}"#,
        r#"{"changes":[{"type":"instance-enable","instanceId":"instance","enabled":"true"}]}"#,
        r#"{"changes":[{"type":"reset","scope":"all","instanceId":"instance"}]}"#,
    ] {
        assert_eq!(
            parse(
                &[
                    "edit",
                    "apply",
                    "/state",
                    "--input",
                    "-",
                    "--if-state",
                    "token"
                ],
                input
            )
            .unwrap_err()
            .exit_code,
            2
        );
    }
    let large = format!(r#"{{"padding":"{}"}}"#, "a".repeat(MAX_COMMAND_BYTES));
    assert!(
        parse(
            &["edit", "actions", "run", "/state", "action", "--input", "-"],
            &large
        )
        .is_err()
    );
}
#[test]
fn observed_operations_require_guards() {
    for command in [
        "edit apply /state --input -",
        "edit rebind /state --input -",
        "edit instances reorder /state --input -",
        "history restore /state --revision rev",
        "styles apply /state style --version v --input -",
        "presets apply /state preset --version v --instance instance --mode update --initialization literal --mask-policy omit",
    ] {
        assert!(
            parse(&command.split_whitespace().collect::<Vec<_>>(), "{}").is_err(),
            "{command}"
        );
    }
}
#[test]
fn values_normalize_and_file_imports_are_not_read_as_command_json() {
    let parsed = parse(
        &[
            "--output",
            "text",
            "edit",
            "set",
            "/state",
            "instance",
            "parameter",
            "--value",
            "[1,2]",
            "--request",
            "req",
        ],
        "",
    )
    .unwrap();
    assert_eq!(parsed.output, OutputMode::Text);
    assert_eq!(
        parsed.invocation.arguments,
        json!({"instanceId":"instance","parameterId":"parameter","value":[1,2],"requestId":"req"})
    );
    let parsed = parse(
        &[
            "resources",
            "import",
            "--kind",
            "icc",
            "--file",
            "/does/not/exist.icc",
        ],
        "",
    )
    .unwrap();
    assert_eq!(parsed.invocation.arguments["file"], "/does/not/exist.icc");
}
#[test]
fn help_version_and_hidden_transport_have_no_execution() {
    assert_eq!(parse(&["--help"], "").unwrap_err().exit_code, 0);
    assert_eq!(parse(&["--version"], "").unwrap_err().exit_code, 0);
    let carrier =
        json!({"operation":"edit show","stateDirectory":"/state","arguments":{}}).to_string();
    assert_eq!(
        parse(&["__invoke"], &carrier).unwrap().invocation.operation,
        "edit show"
    );
    assert!(
        parse(
            &["__invoke"],
            r#"{"operation":"edit show","stateDirectory":"/state","arguments":{"surprise":true}}"#
        )
        .is_err()
    );
}
#[test]
fn errors_preserve_unknown_and_committed_effects() {
    let unknown = ToolError::new("outcome_unknown", "Replay request", 7);
    assert_eq!(unknown.envelope()["error"]["effect"], "unknown");
    let committed = ToolError::new("local_io_failed", "Inspect completed file", 6)
        .effect("partial")
        .detail(json!({"fileCommitted":true,"data":{"path":"/out.tif","fileCommitted":true}}));
    assert_eq!(committed.envelope()["data"]["fileCommitted"], true);
    assert_eq!(committed.envelope()["status"], "partial");
    assert!(committed.envelope()["error"].get("exitCode").is_none());
}
