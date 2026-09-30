#!/usr/bin/env python3
"""Qualify native darktable against an explicit independent RAW reference."""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid

PROTOCOL = "2025-06-18"
LINE_BYTES_MAX = 16 * 1024 * 1024
TOOLS = {"list_modules", "module_schema", "image_parameters", "encode_params", "decode_params", "export_images", "render", "import_images"}
ASSET_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"
EMBEDDED_SHA256 = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
WB_FIELDS = ("red", "green", "blue", "various", "preset")


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def finite_tree(value) -> bool:
    if isinstance(value, float):
        return math.isfinite(value)
    if isinstance(value, dict):
        return all(finite_tree(child) for child in value.values())
    if isinstance(value, list):
        return all(finite_tree(child) for child in value)
    return True


def snapshot_inputs(fixture: Path) -> dict:
    result = {}
    for path in (fixture, fixture.with_suffix(fixture.suffix + ".xmp"), fixture.with_suffix(".xmp")):
        if not path.exists():
            result[path.name] = {"exists": False}
            continue
        info = path.stat()
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("fixture or external XMP is not a regular file")
        result[path.name] = {"exists": True, "sha256": digest(path), "size": info.st_size,
                             "mtime_ns": info.st_mtime_ns, "mode": info.st_mode, "inode": info.st_ino}
    return result


class MCPError(RuntimeError):
    pass


class MCP:
    """Bounded byte transport; an incomplete line cannot bypass the deadline."""
    def __init__(self, command: list[str], root: Path, container: str | None, timeout: float = 180):
        self.container = container
        self.timeout = timeout
        self.seq = 0
        self.buffer = bytearray()
        self.sent = threading.Event()
        self.proc = subprocess.Popen(command, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, bufsize=0,
                                     env={"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root / "xdg"), "OMP_NUM_THREADS": "4"})
        os.set_blocking(self.proc.stdin.fileno(), False)
        os.set_blocking(self.proc.stdout.fileno(), False)

    def send(self, method: str, params: dict, request_id: int | None) -> None:
        message = {"jsonrpc": "2.0", "method": method, "params": params}
        if request_id is not None:
            message["id"] = request_id
        payload = json.dumps(message, allow_nan=False, separators=(",", ":")).encode() + b"\n"
        if len(payload) > LINE_BYTES_MAX:
            raise MCPError("request exceeds transport bound")
        deadline = time.monotonic() + self.timeout
        with selectors.DefaultSelector() as selector:
            selector.register(self.proc.stdin, selectors.EVENT_WRITE)
            pending = memoryview(payload)
            while pending:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise MCPError("request write deadline exceeded")
                try:
                    count = os.write(self.proc.stdin.fileno(), pending)
                except BlockingIOError:
                    continue
                except OSError as exc:
                    raise MCPError("engine request write failed") from exc
                if count <= 0:
                    raise MCPError("engine closed stdin")
                pending = pending[count:]
        self.sent.set()

    def receive(self) -> dict:
        deadline = time.monotonic() + self.timeout
        with selectors.DefaultSelector() as selector:
            selector.register(self.proc.stdout, selectors.EVENT_READ)
            while True:
                newline = self.buffer.find(b"\n")
                if newline >= 0:
                    line = bytes(self.buffer[:newline])
                    del self.buffer[:newline + 1]
                    response = json.loads(line)
                    if not isinstance(response, dict) or not finite_tree(response):
                        raise MCPError("invalid engine response")
                    return response
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise MCPError("response deadline exceeded")
                try:
                    data = os.read(self.proc.stdout.fileno(), min(65536, LINE_BYTES_MAX + 1 - len(self.buffer)))
                except BlockingIOError:
                    continue
                if not data:
                    raise MCPError("engine closed stdout before result")
                self.buffer.extend(data)
                if len(self.buffer) > LINE_BYTES_MAX:
                    raise MCPError("response exceeds transport bound")

    def request(self, method: str, params: dict):
        self.seq += 1
        self.send(method, params, self.seq)
        response = self.receive()
        if response.get("jsonrpc") != "2.0" or response.get("id") != self.seq or "error" in response or "result" not in response:
            raise MCPError("engine returned an invalid request result")
        return response["result"]

    def tool_result(self, name: str, arguments: dict) -> dict:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if not isinstance(result, dict) or type(result.get("isError")) is not bool:
            raise MCPError("engine returned an invalid tool envelope")
        return result

    def tool(self, name: str, arguments: dict):
        result = self.tool_result(name, arguments)
        if result["isError"]:
            raise MCPError("engine rejected " + name)
        content = result.get("content")
        if not isinstance(content, list) or len(content) != 1 or content[0].get("type") != "text":
            raise MCPError("engine returned an invalid text result")
        value = json.loads(content[0]["text"])
        if not finite_tree(value):
            raise MCPError("engine returned nonfinite parameters")
        return value

    def close(self, cancel: bool = False) -> None:
        try:
            if self.proc.poll() is None and not cancel:
                self.request("shutdown", {})
        except (MCPError, OSError, ValueError):
            pass
        finally:
            if self.container:
                subprocess.run(["docker", "rm", "-f", self.container], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=30, check=False)
            if self.proc.poll() is None:
                self.proc.kill()
            self.proc.wait(timeout=30)
            self.proc.stdin.close()
            self.proc.stdout.close()


def prepare_root(out: Path, icc: Path) -> Path:
    root = Path(tempfile.mkdtemp(prefix="native-attempt-", dir=out))
    for name in ("config", "cache", "tmp", "xdg"):
        (root / name).mkdir()
    profile = root / "config/color/out/linear-prophoto.icc"
    profile.parent.mkdir(parents=True)
    shutil.copyfile(icc, profile)
    return root


def start_engine(args, root: Path, fixture: Path) -> tuple[MCP, str, str]:
    image = bool(args.engine_image)
    work = "/work" if image else str(root)
    profile = work + "/config/color/out/linear-prophoto.icc"
    common = ["--core", "--disable-opencl", "--configdir", work + "/config", "--cachedir", work + "/cache",
              "--tmpdir", work + "/tmp", "--library", work + "/library.db", "--conf", "plugins/darkroom/workflow=none",
              "--conf", "write_sidecar_files=never", "--conf", "run_crawler_on_start=FALSE",
              "--conf", "plugins/imageio/format/tiff/bpp=32", "--conf", "plugins/imageio/format/tiff/compress=1"]
    container = "slipstream-native-qualify-" + uuid.uuid4().hex if image else None
    if image:
        command = ["docker", "run", "--name", container, "-i", "--network=none", "--read-only", "--cap-drop=ALL",
                   "--security-opt=no-new-privileges", "--entrypoint", "/opt/darktable/bin/darktable-mcp",
                   "--cpus", "4", "--memory", "4g", "--memory-swap", "4g", "--pids-limit", "256",
                   "--env", "OMP_NUM_THREADS=4", "--mount", f"type=bind,src={fixture.parent},dst=/input,readonly",
                   "--mount", f"type=bind,src={root},dst=/work", args.engine_image, *common]
        source = "/input/" + fixture.name
    else:
        command = [str(Path(args.engine_binary).resolve()), *common]
        source = str(fixture)
    engine = MCP(command, root, container)
    try:
        result = engine.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                "clientInfo": {"name": "slipstream-native-qualification", "version": "1"}})
        if result.get("protocolVersion") != PROTOCOL:
            raise MCPError("wrong protocol version")
        engine.send("notifications/initialized", {}, None)
        return engine, source, profile
    except Exception:
        engine.close(cancel=True)
        raise


def output_argument(args, path: Path) -> str:
    return "/work/" + path.name if args.engine_image else str(path)


def baseline_stack(exposure: float) -> list[dict]:
    return [{"operation": "exposure", "multi_priority": 0, "enabled": True,
             "params": {"mode": "EXPOSURE_MODE_MANUAL", "black": 0.0, "exposure": exposure,
                        "compensate_exposure_bias": False, "compensate_hilite_pres": False}}]


def export_request(source: dict, target: str, profile: str, stack: list, raw: bool = True, edge: int = 0) -> dict:
    request = {"input": source, "out_path": target, "format": "scene-linear-tiff", "icc_file": profile,
               "width": edge, "height": edge, "upscale": False, "high_quality": True, "stack": stack}
    if raw:
        request["baseline"] = "raw-development"
    return request


def export(engine: MCP, request: dict, target: Path) -> None:
    result = engine.tool("export_images", request)
    expected = {"paths": [request["out_path"]], "skipped": 0, "exported": 1, "ok": True}
    if result != expected or not target.is_file():
        raise MCPError("export did not settle the exact requested artifact")


def inspect_tiff(path: Path) -> dict:
    import numpy as np
    import tifffile
    with tifffile.TiffFile(path) as image:
        if len(image.pages) != 1:
            raise MCPError("handoff is not a single frame")
        page = image.pages[0]
        pixels = page.asarray()
        formats = page.tags[339].value
        formats = formats if isinstance(formats, tuple) else (formats,)
        profile = bytes(page.tags[34675].value)
        facts = {"geometry": [page.imagewidth, page.imagelength], "dtype": str(pixels.dtype),
                 "finite": bool(np.isfinite(pixels).all()), "negative": bool((pixels < 0).any()),
                 "overrange": bool((pixels > 1).any()), "minimum": float(pixels.min()), "maximum": float(pixels.max()),
                 "icc_sha256": hashlib.sha256(profile).hexdigest(), "orientation": int(page.tags[274].value)}
        if (pixels.ndim != 3 or pixels.shape[-1] != 3 or page.samplesperpixel != 3 or page.planarconfig != 1
                or pixels.dtype != np.float32 or not facts["finite"] or any(int(x) != 3 for x in formats)
                or int(page.compression) not in (8, 32946) or facts["orientation"] != 1
                or facts["icc_sha256"] != EMBEDDED_SHA256):
            raise MCPError("output failed the pinned TIFF contract")
        return facts


def compare_pixels(native: Path, reference: Path) -> dict:
    import numpy as np
    import tifffile
    a = tifffile.imread(native)
    b = tifffile.imread(reference)
    if a.shape != b.shape or not np.isfinite(a).all() or not np.isfinite(b).all():
        raise MCPError("reference geometry or finite samples differ")
    delta = np.abs(a.astype("float64") - b.astype("float64"))
    result = {"max_abs": float(delta.max()), "mean_abs": float(delta.mean()), "p99_abs": float(np.percentile(delta, 99))}
    result["within_tolerance"] = result["max_abs"] <= 2e-4 and result["p99_abs"] <= 2e-5
    if not result["within_tolerance"]:
        raise MCPError("native pixels differ from independent reference")
    return result


def modules_of(parameters: dict) -> dict:
    return {(module["operation"], module["multi_priority"]): module for module in parameters["modules"]}


def decode(engine: MCP, operation: str, fields: dict, schemas: dict) -> dict:
    encoded = engine.tool("encode_params", {"operation": operation, "fields": fields})
    return engine.tool("decode_params", {"operation": operation, "blob_hex": encoded["blob_hex"],
                       "params_version": schemas[operation]["params_version"]})["fields"]


def generic_probes(engine: MCP, source: str, profile: str, root: Path, args, schemas: dict) -> tuple[dict, dict, int]:
    imported = engine.tool("import_images", {"paths": [source]})
    if imported.get("imported") != 1 or len(imported.get("images", [])) != 1:
        raise MCPError("import did not create the exact private catalog image")
    image_id = imported["images"][0]["imgid"]
    image = {"imgid": image_id}
    before = modules_of(engine.tool("image_parameters", {"input": image}))
    temperature = {key: before[("temperature", 0)]["values"][key] for key in WB_FIELDS}
    if (not all(type(temperature[key]) in (float, int) and math.isfinite(temperature[key])
                and temperature[key] > 0 for key in WB_FIELDS[:3])
            or type(temperature["various"]) not in (float, int)
            or not math.isfinite(temperature["various"]) or temperature["various"] < 0):
        raise MCPError("as-shot initialization lacks actual camera coefficients")
    curve = copy.deepcopy(before[("rgbcurve", 0)]["values"]["curve_nodes"])
    curve[0][1]["y"] = 0.75
    lens = "MCP UTF-8 café 资格"
    if decode(engine, "rgbcurve", {"curve_nodes": curve}, schemas)["curve_nodes"] != curve:
        raise MCPError("nested curve codec changed values")
    if decode(engine, "lens", {"lens": lens}, schemas)["lens"] != lens:
        raise MCPError("UTF-8 string codec changed values")
    target = root / "generic-stack.tiff"
    stack = baseline_stack(0.0) + [
        {"operation": "rgbcurve", "enabled": False, "params": {"curve_nodes": curve}},
        {"operation": "lens", "enabled": False, "params": {"lens": lens}},
        {"operation": "exposure", "multi_priority": 1, "enabled": False, "params": {"exposure": 0.25}},
    ]
    export(engine, export_request(image, output_argument(args, target), profile, stack, edge=256), target)
    after = modules_of(engine.tool("image_parameters", {"input": image}))
    second = after[("exposure", 1)]
    if (second["values"]["exposure"] != 0.25 or second["enabled"]
            or any(value != second["defaults"][key] for key, value in second["values"].items() if key != "exposure")
            or after[("rgbcurve", 0)]["values"]["curve_nodes"] != curve or after[("rgbcurve", 0)]["enabled"]
            or after[("lens", 0)]["values"]["lens"] != lens or after[("lens", 0)]["enabled"]):
        raise MCPError("generic edits or context defaults were not preserved")
    stable = engine.tool("image_parameters", {"input": image})
    blob = engine.tool("encode_params", {"operation": "exposure", "fields": {}})["blob_hex"]
    invalid = {
        "fractional_integer": [{"operation": "rgbcurve", "params": {"curve_num_nodes": [1.5, 2, 2]}}],
        "unknown_enum": [{"operation": "exposure", "params": {"mode": 999}}],
        "null_scalar": [{"operation": "exposure", "params": {"exposure": None}}],
        "string_scalar": [{"operation": "exposure", "params": {"exposure": "bad"}}],
        "partial_bad_stack": [stack[0], {"operation": "not-a-module"}],
        "wrong_blob_version": [{"operation": "exposure", "blob_hex": blob,
                                "params_version": schemas["exposure"]["params_version"] + 1}],
        "illegal_order": [{"operation": "exposure", "before": "rawprepare"}],
    }
    for label, bad in invalid.items():
        rejected_target = root / (label + ".tiff")
        result = engine.tool_result("export_images", export_request(image, output_argument(args, rejected_target), profile, bad))
        if not result["isError"] or rejected_target.exists() or engine.tool("image_parameters", {"input": image}) != stable:
            raise MCPError("invalid intent changed prior image state")
    return {"nested_curve": True, "utf8_string": True, "disabled_edits": True, "second_instance": True,
            "context_defaults_preserved": True, "failure_atomicity": list(invalid)}, temperature, image_id


def run_reference(template: str, fixture: Path, target: Path, exposure: float) -> dict:
    parts = shlex.split(template)
    joined = " ".join(parts)
    if not all(marker in joined for marker in ("{input}", "{output}", "{exposure_milli_ev}")):
        raise ValueError("reference command requires input, output, and exposure_milli_ev placeholders")
    fields = {"input": str(fixture), "output": str(target), "exposure_milli_ev": str(int(exposure * 1000))}
    started = time.monotonic()
    completed = subprocess.run([part.format(**fields) for part in parts], stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=900, check=False)
    if completed.returncode or not target.is_file():
        raise MCPError("independent reference failed")
    metadata = json.loads(target.with_suffix(target.suffix + ".json").read_text())
    temperature = {key: metadata["temperature"][key] for key in WB_FIELDS}
    if not finite_tree(temperature) or not isinstance(metadata["identity"], str):
        raise MCPError("reference parameter evidence is invalid")
    return {"seconds": round(time.monotonic() - started, 3), "identity": metadata["identity"],
            "temperature": temperature, "output": inspect_tiff(target)}


def qualify(args) -> dict:
    import numpy as np
    import tifffile
    fixture = args.fixture.resolve()
    icc = args.icc.resolve()
    if digest(icc) != ASSET_SHA256:
        raise ValueError("wrong pinned ICC asset")
    before = snapshot_inputs(fixture)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {"accepted": False, "protocol": PROTOCOL, "source_sha256": digest(fixture),
              "icc_asset_sha256": ASSET_SHA256, "icc_embedded_sha256": EMBEDDED_SHA256}
    root = prepare_root(out, icc)
    engine, source, profile = start_engine(args, root, fixture)
    try:
        names = {item["name"] for item in engine.request("tools/list", {})["tools"]}
        if TOOLS - names:
            raise MCPError("missing engine tools")
        modules = engine.tool("list_modules", {})
        schemas = {module["operation"]: engine.tool("module_schema", {"operation": module["operation"]})
                   for module in modules if module["have_introspection"]}
        report["discovery"] = {"modules": len(modules), "schemas": len(schemas)}
        report["generic"], temperature, image_id = generic_probes(engine, source, profile, root, args, schemas)
        report["temperature"] = temperature
        for exposure in (0.0, 1.0):
            target = root / f"native-{exposure:g}ev.tiff"
            export(engine, export_request({"imgid": image_id}, output_argument(args, target), profile, baseline_stack(exposure)), target)
            reference = root / f"reference-{exposure:g}ev.tiff"
            ref = run_reference(args.reference_cli, fixture, reference, exposure)
            if ref["temperature"] != temperature:
                raise MCPError("as-shot coefficients differ from independent baseline")
            report.setdefault("references", {})[str(exposure)] = {**ref, "native": inspect_tiff(target),
                                                                 "comparison": compare_pixels(target, reference)}
        synthetic = root / "synthetic-input.tiff"
        pixels = np.tile(np.array([[[-0.25, 0.5, 2.0], [0.0, 1.0, 0.25]]], dtype="float32"), (32, 32, 1))
        profile_bytes = icc.read_bytes()
        tifffile.imwrite(synthetic, pixels, photometric="rgb", compression="deflate",
                         extratags=[(34675, "B", len(profile_bytes), profile_bytes, False)])
        target = root / "synthetic-output.tiff"
        export(engine, export_request({"path": output_argument(args, synthetic)}, output_argument(args, target),
                                     profile, baseline_stack(0.0), raw=False), target)
        report["synthetic"] = inspect_tiff(target)
        if not report["synthetic"]["negative"] or not report["synthetic"]["overrange"]:
            raise MCPError("synthetic scene samples were clipped")
    finally:
        engine.close()
    restart_root = prepare_root(out, icc)
    engine, source, profile = start_engine(args, restart_root, fixture)
    try:
        target = restart_root / "restart.tiff"
        export(engine, export_request({"path": source}, output_argument(args, target), profile, baseline_stack(0.0)), target)
        report["restart"] = compare_pixels(target, root / "native-0ev.tiff")
    finally:
        engine.close()
    cancel_root = prepare_root(out, icc)
    engine, source, profile = start_engine(args, cancel_root, fixture)
    target = cancel_root / "cancelled.tiff"
    errors = []
    responses = []
    engine.sent.clear()
    def attempt():
        try:
            responses.append(engine.tool("export_images", export_request({"path": source}, output_argument(args, target),
                                                                         profile, baseline_stack(1.0))))
        except (MCPError, OSError, ValueError) as exc:
            errors.append(type(exc).__name__)
    thread = threading.Thread(target=attempt)
    thread.start()
    try:
        if not engine.sent.wait(10):
            raise MCPError("cancellation did not submit an engine request")
        deadline = time.monotonic() + 60
        pause = threading.Event()
        while not list(cancel_root.glob("cancelled.tiff.mcp-*")):
            if responses or errors or time.monotonic() >= deadline:
                raise MCPError("cancellation did not observe an in-progress export")
            pause.wait(0.02)
        engine.close(cancel=True)
        thread.join(timeout=10)
        if thread.is_alive() or responses or not errors or target.exists():
            raise MCPError("cancelled attempt reported a completed output")
        report["cancellation"] = {"in_progress_observed": True, "completed_response": False, "artifact_published": False}
    finally:
        if engine.proc.poll() is None:
            engine.close(cancel=True)
        thread.join(timeout=10)
    report["input_unchanged"] = snapshot_inputs(fixture) == before
    if not report["input_unchanged"]:
        raise MCPError("Original or external XMP changed")
    report["accepted"] = True
    (out / "qualification.json").write_text(json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n")
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--engine-binary")
    group.add_argument("--engine-image")
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--reference-cli", required=True)
    parser.add_argument("--icc", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        report = qualify(args)
    except Exception as exc:
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / "qualification.json").write_text(json.dumps({"accepted": False, "error": type(exc).__name__}) + "\n")
        print("qualification failed: " + type(exc).__name__, file=sys.stderr)
        return 2
    print(json.dumps({"accepted": report["accepted"], "source_sha256": report["source_sha256"], "discovery": report["discovery"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
