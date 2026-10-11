"""Exercise real workspace handoff and independent edits on the execution host."""
import argparse
import hashlib
import json
import subprocess
import sys
import uuid
from pathlib import Path


def invoke(binary, *arguments):
    completed = subprocess.run([str(binary), *map(str, arguments)], capture_output=True, text=True)
    try:
        envelope = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"command did not return a JSON envelope: {arguments}: {completed.stderr}") from error
    if completed.returncode or envelope.get("status") != "ok":
        raise RuntimeError(f"command failed: {arguments}: {envelope}")
    return envelope["data"]


def run(arguments):
    from acceptance import validate_development_tiff, validate_png
    steps = []
    def command(name, binary, *args):
        data = invoke(binary, *args)
        steps.append({"name": name, "command": [str(binary), *map(str, args)], "status": "passed"})
        return data
    workspace = command("open-workspace", arguments.slipstream, "workspace", "open", arguments.photo)
    root = Path(workspace["path"])
    if not root.is_dir():
        raise RuntimeError("workspace path must be accessible on this execution host")
    input_path = Path(workspace["inputPath"])
    original = hashlib.sha256(input_path.read_bytes()).hexdigest()
    state = root / ".edits" / "darktable" / ("acceptance-" + uuid.uuid4().hex)
    opened = command("standalone-open", arguments.tool, "open", input_path, "--state", state,
                     "--preview-directory", workspace["previewDirectory"], "--output-directory", workspace["outputDirectory"])
    show = command("inspect-baseline", arguments.tool, "edit", "show", state)
    instances = show["instances"]["items"]
    exposure = [entry for entry in instances if entry["moduleId"] == "exposure"]
    if len(exposure) != 1:
        raise RuntimeError("qualified fixture requires exactly one initialized exposure instance")
    changed = command("save-exposure", arguments.tool, "edit", "set", state, exposure[0]["instanceId"], "exposure",
                      "--value", "0.5", "--if-state", show["stateToken"])
    # Export intentionally precedes every preview; saved settings must already be Film compatible.
    output = command("export-before-preview", arguments.tool, "export", "create", state)
    output_path = Path(output["path"])
    if not output.get("fileCommitted") or output_path.parent != Path(workspace["outputDirectory"]):
        raise RuntimeError("export did not publish in workspace outputs")
    facts, problems = validate_development_tiff(output_path.read_bytes())
    if problems:
        raise RuntimeError(f"export is not a complete scene-linear Film TIFF: {problems}")
    preview = command("preview-after-export", arguments.tool, "preview", "render", state)
    facts_preview, problems = validate_png(Path(preview["path"]).read_bytes(), 4096)
    if problems or Path(preview["path"]).parent != Path(workspace["previewDirectory"]):
        raise RuntimeError(f"preview validation failed: {problems}")
    command("reopen-state", arguments.tool, "edit", "show", state)
    indexed = command("workspace-reconciliation", arguments.slipstream, "workspace", "files", workspace["workspaceId"], "--limit", "60")
    if not any(item.get("filename") == output_path.name for item in indexed["items"]):
        raise RuntimeError("standalone export absent from workspace reconciliation")
    if hashlib.sha256(input_path.read_bytes()).hexdigest() != original:
        raise RuntimeError("workspace input changed")
    # Separate arbitrary-file invocation proves the tool needs no service state or Photo identity.
    outside = arguments.outside_directory.absolute()
    outside.mkdir(mode=0o700)
    independent = command("outside-service-open", arguments.tool, "open", arguments.fixture, "--state", outside / "edit")
    command("outside-service-export", arguments.tool, "export", "create", outside / "edit")
    return {"scope": "workspace-independent-darktable", "status": "passed", "workspace": workspace["workspaceId"],
            "stateDirectory": str(state), "export": output, "tiff": facts, "preview": facts_preview, "steps": steps}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--slipstream", type=Path, default=Path("slipstream"))
    parser.add_argument("--tool", type=Path, default=Path("darktable-edit"))
    parser.add_argument("--photo", required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--outside-directory", type=Path, required=True, help="New directory outside Slipstream state/workspaces")
    parser.add_argument("--i-acknowledge-this-is-an-acceptance-instance", required=True, action="store_true")
    args = parser.parse_args(argv)
    try:
        report = run(args)
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(json.dumps({"scope": "workspace-independent-darktable", "status": "failed", "error": str(error)}))
        return 1
    print(json.dumps(report, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
