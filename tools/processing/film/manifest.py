"""Create the deterministic identity file for the standalone SpektraFilm bundle.

The bundle covers the pinned numerical runtime tree (the patched SpektraFilm
source and the qualified probe modules), the pinned Python runtime tree, the
local film runner, the runtime-generated complete default parameter tree, the
recorded processing-bundle identity, the installed package list, and the
locked requirements. ``bundle`` is the SHA-256 of the emitted
``bundle-manifest.json`` bytes, so the server can verify the manifest
identity and every named asset digest at startup without a second format.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

SOURCE_ROOT = Path("/opt/spektrafilm/src")
RUNTIME_ROOT = Path("/opt/runtime")
PROBE_ROOT = Path("/opt/probe")
OUTPUT = Path("/opt/slipstream-film")
RUNNER = Path("/opt/slipstream-film/runner/film_runner.py")
ENGINE = Path("/opt/runtime/bin/python")
DEFAULT_PARAMETERS = Path("/opt/slipstream-film/parameters-default.json")
PROCESSING_BUNDLE = Path("/opt/processing-bundle.json")
REQUIREMENTS = Path("/opt/requirements.lock")
PACKAGES = Path("/opt/os-packages.txt")

# The shared Film identity this manifest pins field for field; the server
# refuses a bundle whose recipe, handoff, or finished-output identity
# differs from the one the application was qualified against.
RECIPE_SHA256 = "8efdd28d3a49fea7e71835dea82dbc416d9f95e4ae4216ae5fb7535087ec5cf8"
INPUT_ICC_SHA256 = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
OUTPUT_ICC_SHA256 = "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21"
FINISHED_JPEG_QUALITY = 85
PROCEDURE = "film-once-empty-cache-v1"
SOURCE_COMMIT = "3bb2c2d2801ff68b92019cf1dbcbb133d60832bc"


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
    files = [RUNNER, DEFAULT_PARAMETERS, PROCESSING_BUNDLE, REQUIREMENTS, PACKAGES]
    missing = [str(path) for path in files if not path.is_file()]
    missing += [str(ENGINE) if not ENGINE.is_file() else ""]
    missing = [name for name in missing if name]
    if missing:
        raise SystemExit(f"bundle inputs missing: {', '.join(missing)}")
    document = {
        "format": 1,
        "spektrafilm_commit": os.environ["SPEKTRAFILM_COMMIT"],
        "engine": str(ENGINE),
        "runner": str(RUNNER),
        "source_root": str(SOURCE_ROOT),
        "probe_root": str(PROBE_ROOT),
        "recipe_sha256": RECIPE_SHA256,
        "input_icc_sha256": INPUT_ICC_SHA256,
        "output_icc_sha256": OUTPUT_ICC_SHA256,
        "finished_jpeg_quality": FINISHED_JPEG_QUALITY,
        "procedure": PROCEDURE,
        "processing_bundle": digest(PROCESSING_BUNDLE),
        "files": {str(path): digest(path) for path in files},
        "runtime": tree_files(RUNTIME_ROOT),
        "source": tree_files(SOURCE_ROOT),
        "probe": tree_files(PROBE_ROOT),
    }
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    bundle = hashlib.sha256(encoded).hexdigest()
    (OUTPUT / "bundle-manifest.json").write_bytes(encoded)
    (OUTPUT / "bundle").write_text(bundle + "\n")
    print(json.dumps({"bundle": bundle}, sort_keys=True))


if __name__ == "__main__":
    main()
