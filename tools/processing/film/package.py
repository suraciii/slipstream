"""Verify the unchanged numerical runtime; record the separate executor bundle."""

import hashlib
import importlib.metadata
import json
from pathlib import Path
import re
import sys

ROOT = Path("/opt/film-measurement")
EXTRAS = Path("/opt/film-validator-deps")
BUNDLE = ROOT / "bundle.json"
NUMERICAL_SHA256 = "0bf4af15d4f5323d060d4e6543e0e97d1a2fe6014d27c1cd81db440e4f46a152"
NUMERICAL_PARENT_IMAGE = "sha256:10aa79ce1148ba8aec83f6f68e6d39ba7edaa88f2369f21fac65d4784f4b495d"


def file_hash(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identity():
    # Do this on the original interpreter search path, before importing any
    # separate metadata dependency. Extras must not shadow numerical packages.
    sys.path.insert(0, "/opt/probe")
    from bundle import bundle_identity, load_bundle

    if file_hash(Path("/opt/processing-bundle.json")) != NUMERICAL_SHA256:
        raise RuntimeError("Unexpected numerical bundle")
    if bundle_identity() != load_bundle():
        raise RuntimeError("Numerical sources or package inventory changed")
    normalize = lambda name: re.sub(r"[-_.]+", "-", name).lower()
    installed = {normalize(distribution.metadata["Name"])
                 for distribution in importlib.metadata.distributions()}
    extras = {distribution.metadata["Name"]: distribution.version
              for distribution in importlib.metadata.distributions(path=[str(EXTRAS)])}
    if installed.intersection(normalize(name) for name in extras):
        raise RuntimeError("Executor dependency replaces an installed distribution")
    for path in EXTRAS.iterdir():
        if path.name.endswith(".dist-info") or path.name == "__pycache__":
            continue
        module = path.name.split(".")[0]
        if (module in sys.stdlib_module_names
                or any((Path(directory) / path.name).exists()
                       for directory in sys.path if directory)):
            raise RuntimeError("Executor dependency shadows an existing module")
    if any(Path("/work").iterdir()):
        raise RuntimeError("Packaged work directory must be empty")
    for location in ("/opt/spektrafilm", "/opt/probe", "/opt/runtime", "/opt/film-measurement"):
        if any(path.suffix in (".nbc", ".nbi") for path in Path(location).rglob("*")):
            raise RuntimeError("Packaged compiler cache is forbidden")
    files = [ROOT / name for name in (
        "adapter.py", "contract.py", "package.py", "schema.json", "envelope-schema.json",
        "requirements.lock", "Dockerfile",
    )]
    files.append(Path("/usr/local/bin/slipstream-processing-film-worker"))
    extra_files = [path for path in sorted(EXTRAS.rglob("*")) if path.is_file()]
    return {
        "version": 2, "numerical_bundle": NUMERICAL_SHA256,
        "numerical_parent_image": NUMERICAL_PARENT_IMAGE,
        "files_sha256": {str(path): file_hash(path) for path in files},
        "executor_packages": extras,
        "executor_files_sha256": {str(path.relative_to(EXTRAS)): file_hash(path) for path in extra_files},
    }


if __name__ == "__main__":
    actual = identity()
    if sys.argv[1:] == ["--verify"]:
        if actual != json.loads(BUNDLE.read_text()):
            raise RuntimeError("Executor bundle changed")
        print("Numerical and executor bundle identities verified")
    elif not sys.argv[1:]:
        BUNDLE.write_text(json.dumps(actual, sort_keys=True, separators=(",", ":")) + "\n")
        print(file_hash(BUNDLE))
    else:
        raise SystemExit("usage: package.py [--verify]")
