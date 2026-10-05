"""Emit the deterministic manifest for the standalone spektrafilm-rs bundle."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path


OUTPUT = Path("/opt/slipstream-film")
BINARY = OUTPUT / "spektrafilm-f64"
DATA_ROOT = OUTPUT / "data"
RUNTIME_ROOT = OUTPUT / "lib"
DEFAULT_PARAMETERS = OUTPUT / "parameters-default.json"

IMPLEMENTATION = "spektrafilm-rs"
ADAPTER_VERSION = "spektrafilm-rs-adapter-1"
PARAMETER_SCHEMA_VERSION = "spektrafilm-rs-params-1"
FINISHED_JPEG_QUALITY = 85
FILM_PROFILE = "kodak_portra_400"
PRINT_PROFILE = "kodak_portra_endura"


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def tree_files(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): digest(path)
        for path in sorted(root.rglob("*"))
        if path.is_file() and not path.is_symlink()
    }


def main() -> None:
    required = [BINARY, DEFAULT_PARAMETERS]
    missing = [str(path) for path in required if not path.is_file()]
    if not DATA_ROOT.is_dir():
        missing.append(str(DATA_ROOT))
    if not RUNTIME_ROOT.is_dir():
        missing.append(str(RUNTIME_ROOT))
    if missing:
        raise SystemExit(f"bundle inputs missing: {', '.join(missing)}")
    fork_commit = os.environ["SPEKTRAFILM_FORK_COMMIT"]
    if len(fork_commit) != 40 or any(
        character not in "0123456789abcdef" for character in fork_commit
    ):
        raise SystemExit("SPEKTRAFILM_FORK_COMMIT must be a 40-character lowercase commit")
    document = {
        "format": 2,
        "implementation": IMPLEMENTATION,
        "forkReference": "suraciii/spektrafilm-rs",
        "forkCommit": fork_commit,
        "adapterVersion": ADAPTER_VERSION,
        "parameterSchemaVersion": PARAMETER_SCHEMA_VERSION,
        "binary": str(BINARY),
        "dataRoot": str(DATA_ROOT),
        "parametersDefault": str(DEFAULT_PARAMETERS),
        "filmProfile": FILM_PROFILE,
        "printProfile": PRINT_PROFILE,
        "finishedJpegQuality": FINISHED_JPEG_QUALITY,
        "files": {
            **{str(path): digest(path) for path in required},
            **{
                str(path): digest(path)
                for path in sorted(RUNTIME_ROOT.rglob("*"))
                if path.is_file()
            },
        },
        "data": tree_files(DATA_ROOT),
    }
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    bundle = hashlib.sha256(encoded).hexdigest()
    OUTPUT.joinpath("bundle-manifest.json").write_bytes(encoded)
    OUTPUT.joinpath("bundle").write_text(bundle + "\n")
    print(json.dumps({"bundle": bundle}, sort_keys=True))


if __name__ == "__main__":
    main()
