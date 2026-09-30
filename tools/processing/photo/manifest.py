"""Create the deterministic identity file for the native photo image."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

NATIVE_ROOT = Path("/opt/darktable")
OUTPUT = Path("/opt/slipstream-photo")
FILES = (
    Path("/usr/local/bin/slipstream-processing-photo-worker"),
    Path("/opt/slipstream-photo/engine-metadata.json"),
    Path("/opt/slipstream-photo/film_adapter.py"),
    Path("/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc"),
    Path("/opt/processing-bundle.json"),
    Path("/opt/probe/bundle.py"),
    Path("/opt/probe/film_identity.py"),
    Path("/opt/probe/finished_jpeg.py"),
    Path("/opt/probe/film.py"),
    Path("/opt/os-packages.txt"),
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
    files = native_files() + list(FILES)
    missing = [str(path) for path in files if not path.is_file()]
    if missing:
        raise SystemExit(f"bundle inputs missing: {', '.join(missing)}")
    native = {str(path.relative_to(NATIVE_ROOT)): digest(path) for path in native_files()}
    document = {
        "format": 1,
        "darktable_commit": os.environ["DARKTABLE_COMMIT"],
        "native": native,
        "files": {str(path): digest(path) for path in FILES},
        "metadata": digest(FILES[1]),
        "icc": digest(FILES[3]),
        "film_parent_manifest": digest(FILES[4]),
    }
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    bundle = hashlib.sha256(encoded).hexdigest()
    (OUTPUT / "bundle-manifest.json").write_bytes(encoded)
    (OUTPUT / "bundle").write_text(bundle + "\n")
    print(json.dumps({"bundle": bundle, **document}, sort_keys=True))


if __name__ == "__main__":
    main()
