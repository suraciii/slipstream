"""Discover the production darktable-mcp contract during image build."""

from __future__ import annotations

import json
import os
import selectors
import subprocess
import sys
import tempfile
import time
from pathlib import Path


LINE_BYTES_MAX = 16 * 1024 * 1024
TIMEOUT = 8.0


def request(process: subprocess.Popen[bytes], selector: selectors.BaseSelector, request_id: int, method: str, params: dict) -> dict:
    payload = json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}, separators=(",", ":")).encode() + b"\n"
    if len(payload) > 1 << 20:
        raise ValueError(f"request for {method} exceeds the discovery bound")
    deadline = time.monotonic() + TIMEOUT
    selector.register(process.stdin, selectors.EVENT_WRITE)
    try:
        pending = memoryview(payload)
        while pending:
            events = selector.select(max(0.05, deadline - time.monotonic()))
            if not events:
                raise TimeoutError(f"timed out writing darktable-mcp {method}")
            for key, _ in events:
                if key.fileobj is process.stdin:
                    try:
                        written = os.write(process.stdin.fileno(), pending)
                    except BlockingIOError:
                        continue
                    pending = pending[written:]
                    break
    finally:
        selector.unregister(process.stdin)
    while time.monotonic() < deadline:
        events = selector.select(max(0.05, deadline - time.monotonic()))
        for key, _ in events:
            line = key.fileobj.readline(LINE_BYTES_MAX + 1)
            if not line:
                raise RuntimeError("darktable-mcp closed stdout before replying")
            if len(line) > LINE_BYTES_MAX or not line.endswith(b"\n"):
                raise RuntimeError("darktable-mcp emitted an oversized or incomplete line")
            try:
                response = json.loads(line)
            except json.JSONDecodeError as error:
                raise RuntimeError(f"darktable-mcp emitted invalid JSON: {line!r}") from error
            if response.get("id") == request_id:
                if "error" in response:
                    raise RuntimeError(f"darktable-mcp {method} failed: {response['error']}")
                result = response.get("result")
                if not isinstance(result, dict):
                    raise RuntimeError(f"darktable-mcp {method} returned no result object")
                return result
    raise TimeoutError(f"timed out waiting for darktable-mcp {method}")


def decoded_tool_payload(result: dict, method: str) -> object:
    """Decode one exact MCP text result for native schema admission."""
    if result.get("isError") is not False:
        raise RuntimeError(f"darktable-mcp {method} did not report isError=false")
    content = result.get("content")
    if not isinstance(content, list) or len(content) != 1:
        raise RuntimeError(f"darktable-mcp {method} returned non-exact content")
    item = content[0]
    if not isinstance(item, dict) or item.get("type") != "text" or set(item) != {"type", "text"}:
        raise RuntimeError(f"darktable-mcp {method} returned non-text content")
    try:
        return json.loads(item["text"])
    except (TypeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"darktable-mcp {method} returned invalid JSON text") from error


def normalize_module_entries(payload: object) -> list[dict]:
    if isinstance(payload, list):
        entries = payload
    elif isinstance(payload, dict) and isinstance(payload.get("modules"), list):
        entries = payload["modules"]
    else:
        raise RuntimeError("darktable-mcp list_modules payload has no modules array")
    if any(
        not isinstance(entry, dict)
        or not isinstance(entry.get("operation"), str)
        or not entry["operation"]
        for entry in entries
    ):
        raise RuntimeError("darktable-mcp list_modules contains an invalid module")
    operations = [entry["operation"] for entry in entries]
    if len(operations) != len(set(operations)):
        raise RuntimeError("darktable-mcp list_modules contains duplicate operations")
    return entries


def main() -> None:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "/opt/darktable/bin/darktable-mcp")
    metadata = Path(os.environ.get("ENGINE_METADATA", "/opt/slipstream-photo/engine-metadata.json"))
    root = Path("/work/config")
    (root / "darktable").mkdir(parents=True, exist_ok=True)
    (root / "cache").mkdir(parents=True, exist_ok=True)
    diagnostics_stream = tempfile.TemporaryFile()
    process = subprocess.Popen(
        [str(binary), "--core", "--configdir", str(root / "darktable"), "--cachedir", str(root / "cache")],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=diagnostics_stream,
        start_new_session=True,
    )
    os.set_blocking(process.stdin.fileno(), False)
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    try:
        initialize = request(process, selector, 0, "initialize", {})
        if initialize.get("protocolVersion") != "2025-06-18":
            raise RuntimeError(f"unexpected MCP protocol: {initialize}")
        os.write(process.stdin.fileno(), b'{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}\n')
        tools_result = request(process, selector, 1, "tools/list", {})
        tools = tools_result.get("tools")
        if not isinstance(tools, list):
            raise RuntimeError("darktable-mcp tools/list returned no tools array")
        modules_result = request(
            process, selector, 2, "tools/call",
            {"name": "list_modules", "arguments": {}},
        )
        normalized_modules = normalize_module_entries(
            decoded_tool_payload(modules_result, "list_modules")
        )
        schemas = {}
        for index, entry in enumerate(normalized_modules, 3):
            if entry.get("have_introspection"):
                schema_result = request(
                    process, selector, index, "tools/call",
                    {"name": "module_schema", "arguments": {"operation": entry["operation"]}},
                )
                schemas[entry["operation"]] = decoded_tool_payload(schema_result, "module_schema")
        document = {
            "protocol": initialize["protocolVersion"],
            "binary": str(binary),
            "tools": tools,
            "modules": normalized_modules,
            "operations": sorted(entry["operation"] for entry in normalized_modules),
            "schemas": schemas,
        }
        metadata.parent.mkdir(parents=True, exist_ok=True)
        metadata.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n")
    finally:
        try:
            request(process, selector, 100000, "shutdown", {})
        except (BrokenPipeError, RuntimeError, TimeoutError):
            pass
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
        diagnostics_stream.seek(0)
        diagnostics = diagnostics_stream.read().decode(errors="replace")
        diagnostics_stream.close()
        if process.returncode not in (0, -15):
            raise RuntimeError(f"darktable-mcp discovery failed ({process.returncode}): {diagnostics}")
        if diagnostics:
            print(diagnostics, file=sys.stderr, end="")


if __name__ == "__main__":
    main()
