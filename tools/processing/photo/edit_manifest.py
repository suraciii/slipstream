"""Create the independent tool's exact, fully digest-pinned engine bundle."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path

ROOT = Path("/opt/darktable-edit")


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def create_manifest(root, metadata, commit):
    discovery = metadata["discovery"]
    profiles = discovery["outputProfiles"]
    if not isinstance(profiles, dict) or not profiles or any(not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value) for value in profiles.values()):
        raise ValueError("native discovery must disclose exact embedded ICC SHA-256 values")
    files = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"independent bundle forbids symlinks: {path}")
        if path.is_file() and path.relative_to(root).as_posix() not in {"edit-manifest.json", "bundle"}:
            files[path.relative_to(root).as_posix()] = digest(path)
    binary, data = "native/bin/darktable-mcp", "native/share/darktable"
    if binary not in files or not (root / data).is_dir():
        raise ValueError("independent bundle native binary/data are missing")
    document = {"format": 1, "schemaVersion": discovery["schemaVersion"],
                "protocolVersion": metadata["protocol"], "binary": binary,
                "dataDirectory": data, "files": files, "tools": metadata["tools"],
                "discovery": discovery, "outputProfiles": profiles}
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    (root / "edit-manifest.json").write_bytes(encoded)
    identity = hashlib.sha256(encoded).hexdigest()
    (root / "bundle").write_text(identity + "\n")
    return {"bundle": identity, "darktableCommit": commit}


def main():
    metadata = json.loads((ROOT / "engine-metadata.json").read_bytes())
    commit = os.environ["DARKTABLE_COMMIT"]
    if len(commit) != 40 or any(c not in "0123456789abcdef" for c in commit):
        raise ValueError("full committed native revision required")
    (ROOT / "darktable-commit").write_text(commit + "\n")
    print(json.dumps(create_manifest(ROOT, metadata, commit), sort_keys=True))


if __name__ == "__main__":
    main()
