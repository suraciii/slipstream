"""Qualification checks use synthetic inputs only and do not require a camera."""

import hashlib
import json
from pathlib import Path
import os
import subprocess
import sys


def run(*args, **kwargs):
    subprocess.run([sys.executable, *args], check=True, **kwargs)


# Test-only packages are installed outside the pinned runtime. Verify that
# runtime's identity in a child which does not add the test-only import path.
run("/opt/probe/bundle.py", "--verify",
    env={**os.environ, "PYTHONPATH": "/opt/spektrafilm/src"})
print(json.dumps({"test_patch_sha256": hashlib.sha256(
    Path("/opt/engine_checks/0001-current-grain-contract.patch").read_bytes()
).hexdigest()}), flush=True)
run("/opt/engine_checks/test_bounded_gamut.py")
for count in (1, 65536, 262144):
    for content in ("neutral", "mixed"):
        run("/opt/engine_checks/scratch_probe.py", str(count), content)
run("-m", "pytest", "-q", "-p", "no:cacheprovider",
    "tests/test_gamut_compression.py", "tests/test_runtime_api.py",
    "tests/test_topology.py", "tests/test_photo_params.py", "tests/test_lut_mode.py",
    cwd="/opt/spektrafilm")
