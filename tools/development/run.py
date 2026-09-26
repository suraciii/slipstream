"""Stage an opt-in fixture and run a CPU-only, offline qualification container."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time
import uuid


def snapshot(path):
    with path.open("rb") as source:
        before = os.fstat(source.fileno())
        digest = hashlib.file_digest(source, "sha256").hexdigest()
        after = os.fstat(source.fileno())
    fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_mode", "st_uid", "st_gid")
    assert all(getattr(before, field) == getattr(after, field) for field in fields), "Source changed while hashing"
    return {"sha256": digest, **{field: getattr(after, field) for field in fields}}


def main():
    if not __debug__:
        raise RuntimeError("Qualification safety checks require Python assertions")
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New directory outside the repository")
    parser.add_argument("--image", default="slipstream:development-qualification")
    parser.add_argument("--mode", choices=("smoke", "benchmark", "full"), default="smoke")
    parser.add_argument("--stage", choices=("raw", "pipeline"), default="pipeline")
    parser.add_argument("--repetitions", type=int, default=20)
    args = parser.parse_args()
    source = args.raw.resolve(strict=True)
    output = args.output.resolve()
    repository = Path(__file__).resolve().parents[2]
    if not source.is_file() or output.is_relative_to(repository) or "," in str(output):
        parser.error("use a regular RAW fixture and an output directory outside the repository without commas")
    if args.repetitions < 2:
        parser.error("at least two warm repetitions are required")
    image = json.loads(subprocess.check_output(["docker", "image", "inspect", args.image]))[0]
    identity = image["Id"]
    original = snapshot(source)
    sidecar_paths = {source.with_suffix(".xmp"), source.with_suffix(".XMP"), Path(str(source) + ".xmp")}
    sidecars = {path: snapshot(path) for path in sidecar_paths if path.is_file()}
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    staged = output / "input"
    staged.mkdir(mode=0o700)
    work = output / "work"
    work.mkdir(mode=0o700)
    input_name = "sample" + source.suffix.lower()
    with source.open("rb") as reader, (staged / input_name).open("xb") as writer:
        shutil.copyfileobj(reader, writer)
    assert snapshot(source) == original
    assert snapshot(staged / input_name)["sha256"] == original["sha256"]
    (staged / input_name).chmod(0o444)
    name = f"slipstream-development-{uuid.uuid4().hex}"
    command = [
        "docker", "create", "--name", name, "--user", f"{os.getuid()}:{os.getgid()}",
        "--network", "none", "--cpus", "4", "--memory", "32g", "--memory-swap", "32g",
        "--pids-limit", "256", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "--read-only", "--tmpfs", "/tmp:rw,size=2g", "-e", f"PROBE_RAW_NAME={input_name}",
        "--mount", f"type=bind,src={staged},dst=/input,readonly",
        "--mount", f"type=bind,src={work},dst=/work",
        identity, "--mode", args.mode, "--stage", args.stage,
        "--repetitions", str(args.repetitions),
    ]
    started = time.monotonic()
    timing = {
        "admission_seconds": None,
        "startup_seconds": None,
        "execution_seconds": None,
        "settlement_seconds": None,
    }
    result = None
    process = None
    inspected = None
    admission_pending = True
    interrupted_signal = None
    def interrupted(signum, frame):
        nonlocal interrupted_signal
        interrupted_signal = signum
        # Admission creates a stopped container. Settle it before cancellation
        # inspection; otherwise the container could appear after cleanup.
        if not admission_pending:
            raise KeyboardInterrupt(f"Received signal {signum}")
    previous_signals = {s: signal.signal(s, interrupted) for s in (signal.SIGTERM, signal.SIGINT)}
    try:
        admission_started = time.monotonic()
        try:
            admission = subprocess.run(command, capture_output=True, text=True,
                                       start_new_session=True, check=True)
        finally:
            timing["admission_seconds"] = time.monotonic() - admission_started
        container_id = admission.stdout.strip()
        admission_pending = False
        if interrupted_signal is not None:
            raise KeyboardInterrupt(f"Received signal {interrupted_signal} during admission")
        admission_pending = True
        startup_started = time.monotonic()
        try:
            subprocess.run(["docker", "start", container_id], check=True,
                           stdout=subprocess.DEVNULL, start_new_session=True)
        finally:
            timing["startup_seconds"] = time.monotonic() - startup_started
        admission_pending = False
        if interrupted_signal is not None:
            raise KeyboardInterrupt(f"Received signal {interrupted_signal} during start")
        execution_started = time.monotonic()
        try:
            with (output / "probe.jsonl").open("x") as log:
                process = subprocess.Popen(["docker", "logs", "--follow", container_id],
                                           stdout=log, stderr=subprocess.STDOUT,
                                           start_new_session=True)
                terminal = subprocess.run(["docker", "wait", container_id], capture_output=True,
                                          text=True, check=True, start_new_session=True)
                code = int(terminal.stdout.strip())
                process.wait()
                result = subprocess.CompletedProcess(command, code)
        finally:
            timing["execution_seconds"] = time.monotonic() - execution_started
    finally:
        # docker run's CLI process does not own the daemon's container lifetime.
        # Inspect/stop only this invocation's randomly named container, even on
        # SIGTERM or a Python exception, and retain its terminal state first.
        for sig in previous_signals:
            signal.signal(sig, signal.SIG_IGN)
        errors = []
        def attempt(label, action):
            try:
                return action()
            except Exception as error:
                errors.append(f"{label}: {type(error).__name__}: {error}")
                return None
        settlement_started = time.monotonic()
        try:
            inspected = attempt("inspect container", lambda: json.loads(
                subprocess.check_output(["docker", "inspect", name], stderr=subprocess.PIPE))[0])
            if inspected and inspected["State"]["Running"]:
                attempt("stop container", lambda: subprocess.run(
                    ["docker", "stop", "--time", "5", name], check=True,
                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE))
                inspected = attempt("inspect terminal state", lambda: json.loads(
                    subprocess.check_output(["docker", "inspect", name], stderr=subprocess.PIPE))[0])
            running_or_unknown = inspected is None or inspected["State"]["Running"]
            if process is not None:
                if running_or_unknown and process.poll() is None:
                    # This is only the log observer; terminating it does not claim
                    # that the daemon-owned container has stopped.
                    attempt("stop log observer", process.terminate)
                attempt("settle log observer", process.wait)
            original_after = attempt("verify Original", lambda: snapshot(source))
            sidecars_after = {path: attempt("verify external XMP", lambda p=path: snapshot(p))
                              for path in sidecars}
            removed = False
            if not running_or_unknown:
                removal = attempt("remove stopped container", lambda: subprocess.run(
                    ["docker", "rm", name], check=True, stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE))
                removed = removal is not None
            else:
                errors.append("Container settlement is unconfirmed; retained container: " + name)
            timing["settlement_seconds"] = time.monotonic() - settlement_started
            complete_runner_seconds = time.monotonic() - started
            report = {
                "image_id": identity, "mode": args.mode, "stage": args.stage,
                "exit_code": result.returncode if result else None,
                "interrupted_signal": interrupted_signal,
                "container_state": inspected["State"] if inspected else None,
                "retained_container": None if removed else name,
                "seconds": complete_runner_seconds,
                "latency": {
                    **timing,
                    "complete_runner_seconds": complete_runner_seconds,
                    "scope": "host qualification runner from docker create through source checks",
                    "production_request_latency": False,
                },
                "source_sha256": original["sha256"], "source_bytes": original["st_size"],
                "source_unchanged": original_after == original if original_after else None,
                "sidecars_unchanged": all(sidecars_after[p] == v for p, v in sidecars.items()),
                "qualification": "incomplete", "settlement_errors": errors,
            }
            attempt("write report", lambda: (output / "report.json").write_text(json.dumps(report, indent=2)))
            attempt("print report", lambda: print(json.dumps(report, indent=2)))
            if errors or not report["source_unchanged"] or not report["sidecars_unchanged"]:
                raise RuntimeError("Qualification settlement or source verification failed: " + "; ".join(errors))
        finally:
            for sig, previous in previous_signals.items():
                signal.signal(sig, previous)
    raise SystemExit(result.returncode if result is not None else 1)


if __name__ == "__main__":
    main()
