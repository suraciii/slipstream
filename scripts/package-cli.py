#!/usr/bin/env python3
"""Build a non-published, source-bound Linux amd64 CLI candidate."""

import gzip
import hashlib
import json
import os
import platform
import stat
import subprocess
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FILES = {
    "LICENSE": "LICENSE",
    "THIRD-PARTY-NOTICES.md": "THIRD-PARTY-NOTICES.md",
    "RUST-LICENSES.html": "RUST-LICENSES.html",
    "docs/cli-install.md": "docs/cli-install.md",
    "docs/cli-reference.md": "docs/cli-reference.md",
    "docs/command-line.md": "docs/command-line.md",
    "docs/access.md": "docs/access.md",
    "docs/library-browsing-and-selection.md": "docs/library-browsing-and-selection.md",
    "design/preview-pipeline.md": "design/preview-pipeline.md",
    "docs/agent-cli.md": "docs/agent-cli.md",
}


def run(*command):
    result = subprocess.run(
        command, cwd=ROOT, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def file_info(path, name):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"candidate input is not a regular file: {path}")
    info = tarfile.TarInfo(name)
    info.size = metadata.st_size
    info.mode = 0o755 if name.endswith("/slipstream") else 0o644
    info.mtime = 0
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    return info


def main():
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("the CLI candidate target is Linux amd64")
    if run("git", "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("commit the candidate before packaging it")
    metadata = run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1")
    packages = json.loads(metadata)["packages"]
    version = next(
        package["version"] for package in packages if package["name"] == "slipstream-cli"
    )
    commit = run("git", "rev-parse", "HEAD")
    if len(commit) != 40 or any(character not in "0123456789abcdef" for character in commit):
        raise ValueError("expected one full Git commit ID")
    run(
        "cargo", "build", "--release", "--locked", "-p", "slipstream-cli", "--bin", "slipstream"
    )
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = (target if target.is_absolute() else ROOT / target) / "release" / "slipstream"
    if run(str(binary), "--version") != f"slipstream {version}":
        raise ValueError("the built client version does not match Cargo metadata")

    name = f"slipstream-cli-{version}-g{commit[:12]}-linux-amd64"
    dist = ROOT / "dist"
    if dist.is_symlink():
        raise ValueError("the output directory must not be a symbolic link")
    dist.mkdir(exist_ok=True)
    output = dist / name
    output.mkdir()
    archive = output / f"{name}.tar.gz"
    with archive.open("xb") as stream:
        with gzip.GzipFile(fileobj=stream, mode="wb", filename="", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as tar:
                sources = [(binary, "slipstream")]
                sources.extend((ROOT / path, member) for path, member in FILES.items())
                for source, member in sources:
                    info = file_info(source, f"{name}/{member}")
                    with source.open("rb") as data:
                        tar.addfile(info, data)
    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    checksum = output / f"{archive.name}.sha256"
    checksum.write_text(f"{digest.hexdigest()}  {archive.name}\n", encoding="ascii")
    print(output)


if __name__ == "__main__":
    main()
