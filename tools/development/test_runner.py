"""Host-side lifecycle regressions. Engine/color checks require the real probe."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

RUNNER = Path(__file__).with_name("run.py")
FAKE_DOCKER = r'''#!/usr/bin/env python3
import json,os,sys,time
from pathlib import Path
root=Path(os.environ["FAKE_DOCKER_STATE"])
args=sys.argv[1:]
if args[:2]==["image","inspect"]:
 print(json.dumps([{"Id":"sha256:qualified-image"}]))
elif args[0]=="create":
 assert args[-7]=="sha256:qualified-image", args
 if os.environ["FAKE_MODE"]=="interrupt-before-create":
  (root/"admitting").write_text("yes")
  while not (root/"admission-release").exists():time.sleep(0.01)
 (root/"container.json").write_text(json.dumps({"State":{"Running":False,"OOMKilled":False,"ExitCode":0}}))
 print("created-container-id")
elif args[0]=="start":
 if os.environ["FAKE_MODE"]=="interrupt-before-start":
  (root/"starting").write_text("yes")
  while not (root/"admission-release").exists():time.sleep(0.01)
 (root/"container.json").write_text(json.dumps({"State":{"Running":True,"OOMKilled":False,"ExitCode":0}}))
 (root/"started").write_text("yes")
 if os.environ["FAKE_MODE"]=="fail":
  (root/"container.json").write_text(json.dumps({"State":{"Running":False,"OOMKilled":True,"ExitCode":137}}))
elif args[0] in ["wait","logs"]:
 while json.loads((root/"container.json").read_text())["State"]["Running"]:time.sleep(0.01)
 if args[0]=="wait":print(json.loads((root/"container.json").read_text())["State"]["ExitCode"])
elif args[0]=="inspect":
 print('['+(root/"container.json").read_text()+']')
elif args[0]=="stop":
 if os.environ["FAKE_MODE"]=="interrupt-stop-error":
  sys.exit(1)
 (root/"container.json").write_text(json.dumps({"State":{"Running":False,"OOMKilled":False,"ExitCode":143}}))
 (root/"stopped").write_text("yes")
elif args[0]=="rm":
 assert not json.loads((root/"container.json").read_text())["State"]["Running"]
 (root/"removed").write_text("yes")
else:raise RuntimeError(args)
'''


class RunnerTests(unittest.TestCase):
    def run_case(self, mode):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            executable = root / "docker"
            executable.write_text(FAKE_DOCKER)
            executable.chmod(0o755)
            raw = root / "original.ARW"
            raw.write_bytes(b"private fixture bytes")
            sidecar = root / "original.xmp"
            sidecar.write_bytes(b"private external XMP")
            original_stat = raw.stat()
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                   "FAKE_DOCKER_STATE": str(root), "FAKE_MODE": mode}
            process = subprocess.Popen(
                [sys.executable, str(RUNNER), "--raw", str(raw),
                 "--output", str(root / "output"), "--repetitions", "2"],
                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            )
            try:
                if mode.startswith("interrupt"):
                    deadline = time.monotonic() + 5
                    marker = {"interrupt-before-create": "admitting",
                              "interrupt-before-start": "starting"}.get(mode, "started")
                    while not (root / marker).exists():
                        if process.poll() is not None or time.monotonic() >= deadline:
                            self.fail("Runner did not launch the controlled container")
                        time.sleep(0.01)
                    process.send_signal(signal.SIGTERM)
                    if mode in ("interrupt-before-create", "interrupt-before-start"):
                        (root / "admission-release").write_text("continue admission")
                stdout, stderr = process.communicate(timeout=5)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
            self.assertNotEqual(process.returncode, 0, stdout + stderr)
            self.assertEqual((root / "removed").exists(), mode != "interrupt-stop-error", stdout + stderr)
            report = json.loads((root / "output/report.json").read_text())
            self.assertEqual(report["container_state"]["Running"], mode == "interrupt-stop-error")
            self.assertEqual(report["container_state"]["OOMKilled"], mode == "fail")
            self.assertEqual(report["container_state"]["ExitCode"],
                             {"fail": 137, "interrupt": 143, "interrupt-before-create": 0,
                              "interrupt-before-start": 143, "interrupt-stop-error": 0}[mode])
            if mode == "interrupt-stop-error":
                self.assertTrue(report["settlement_errors"])
                self.assertIsNotNone(report["retained_container"])
            if mode == "interrupt-before-create":
                self.assertFalse((root / "started").exists(), "Cancelled admission must not start heavy work")
            self.assertTrue(report["source_unchanged"])
            self.assertTrue(report["sidecars_unchanged"])
            latency = report["latency"]
            self.assertFalse(latency["production_request_latency"])
            self.assertEqual(report["seconds"], latency["complete_runner_seconds"])
            phase_seconds = [
                latency[name]
                for name in (
                    "admission_seconds", "startup_seconds",
                    "execution_seconds", "settlement_seconds",
                )
                if latency[name] is not None
            ]
            self.assertGreaterEqual(latency["complete_runner_seconds"], 0)
            self.assertTrue(all(value >= 0 for value in phase_seconds))
            self.assertGreaterEqual(
                latency["complete_runner_seconds"],
                sum(phase_seconds),
            )
            self.assertEqual(raw.read_bytes(), b"private fixture bytes")
            self.assertEqual(raw.stat().st_mtime_ns, original_stat.st_mtime_ns)
            self.assertEqual(sidecar.read_bytes(), b"private external XMP")
    def test_failed_container_retains_failure_and_originals(self):
        self.run_case("fail")

    def test_sigterm_settles_container_and_preserves_originals(self):
        self.run_case("interrupt")

    def test_sigterm_during_admission_never_starts_heavy_work(self):
        self.run_case("interrupt-before-create")

    def test_sigterm_during_start_stops_the_admitted_container(self):
        self.run_case("interrupt-before-start")

    def test_failed_stop_retains_unknown_work_and_source_evidence(self):
        self.run_case("interrupt-stop-error")


if __name__ == "__main__":
    unittest.main()
