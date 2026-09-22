"""Build and verify the immutable qualification bundle's source identities."""

import hashlib
import importlib.metadata
import json
from pathlib import Path
import sys

SOURCE_COMMIT = "3bb2c2d2801ff68b92019cf1dbcbb133d60832bc"
SOURCE_ARCHIVE_SHA256 = "b2aca043227bb76f7f09eef4466cd8f9410f0cd5832ae2f4d045b80cb13b328f"
ROOT = Path("/opt/spektrafilm")
MANIFEST = Path("/opt/processing-bundle.json")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def bundle_identity():
    data = ROOT / "src/spektrafilm/data"
    assets = {str(path.relative_to(data)): digest(path)
              for path in sorted(data.rglob("*")) if path.is_file()}
    files = [
        Path("/opt/requirements.lock"), Path("/opt/os-packages.txt"),
        Path("/opt/development-Dockerfile"), Path("/opt/probe/film.py"),
        Path("/opt/probe/probe.py"), Path("/opt/probe/bundle.py"),
        Path("/opt/patches/0001-bounded-output-gamut.patch"),
        *[ROOT / "src/spektrafilm" / path for path in (
            "utils/bounded_gamut.py", "runtime/process.py",
            "runtime/pipeline.py", "runtime/stages/scanning.py",
        )],
    ]
    return {
        "schema": 1,
        "source_commit": SOURCE_COMMIT,
        "source_archive_sha256": SOURCE_ARCHIVE_SHA256,
        "patches": ["0001-bounded-output-gamut.patch"],
        "files_sha256": {str(path): digest(path) for path in files},
        "profile_assets_sha256": hashlib.sha256(
            json.dumps(assets, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "profile_asset_count": len(assets),
        "python": sys.version.split()[0],
        "packages": {d.metadata["Name"]: d.version
                     for d in importlib.metadata.distributions()},
    }


def load_bundle():
    return json.loads(MANIFEST.read_text())


if __name__ == "__main__":
    actual = bundle_identity()
    if sys.argv[1:] == ["--verify"]:
        if actual != load_bundle():
            raise RuntimeError("Processing bundle differs from its recorded identity")
        print("Processing bundle identity verified")
    elif sys.argv[1:]:
        raise SystemExit("usage: bundle.py [--verify]")
    else:
        MANIFEST.write_text(json.dumps(actual, indent=2, sort_keys=True) + "\n")
