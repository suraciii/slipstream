"""Create the deterministic identity file for the native photo bundle.

The bundle covers the native engine tree, the discovered engine metadata, the
ICC output profile, the native source commit, and the installed package list.
``bundle`` is the SHA-256 of the emitted ``bundle-manifest.json`` bytes, so
the server can verify the manifest identity and every named asset digest at
startup without a second format.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

NATIVE_ROOT = Path("/opt/darktable")
OUTPUT = Path("/opt/slipstream-photo")
ENGINE = Path("/opt/darktable/bin/darktable-mcp")
METADATA = Path("/opt/slipstream-photo/engine-metadata.json")
ICC = Path("/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc")
COMMIT = Path("/opt/slipstream-photo/darktable-commit")
PACKAGES = Path("/opt/os-packages.txt")
FILES = (
    METADATA,
    ICC,
    COMMIT,
    PACKAGES,
)

def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def native_files() -> list[Path]:
    return sorted(path for path in NATIVE_ROOT.rglob("*") if path.is_file())


def main() -> None:
    files = native_files() + list(FILES) + [ENGINE]
    missing = [str(path) for path in files if not path.is_file()]
    if missing:
        raise SystemExit(f"bundle inputs missing: {', '.join(missing)}")
    native = {str(path.relative_to(NATIVE_ROOT)): digest(path) for path in native_files()}
    document = {
        "format": 1,
        "darktable_commit": os.environ["DARKTABLE_COMMIT"],
        "engine": str(ENGINE),
        "native": native,
        "files": {str(path): digest(path) for path in FILES},
        "metadata": digest(METADATA),
        "icc": digest(ICC),
    }
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    bundle = hashlib.sha256(encoded).hexdigest()
    (OUTPUT / "bundle-manifest.json").write_bytes(encoded)
    (OUTPUT / "bundle").write_text(bundle + "\n")
    print(json.dumps({"bundle": bundle, **document}, sort_keys=True))


if __name__ == "__main__":
    main()
