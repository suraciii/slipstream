#!/usr/bin/env python3
"""Build a non-published, source-bound Linux amd64 CLI candidate."""

import argparse
import importlib.util
import sys
import configparser
import gzip
import hashlib
import io
import json
import os
import platform
import posixpath
import re
import stat
import subprocess
import tarfile
from pathlib import Path
from urllib.parse import quote, unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent
FILES = {
    "LICENSE": "LICENSE",
    "THIRD-PARTY-NOTICES.md": "THIRD-PARTY-NOTICES.md",
    "RUST-LICENSES.html": "RUST-LICENSES.html",
    "docs/cli-install.md": "docs/cli-install.md",
    "CONTEXT.md": "CONTEXT.md",
    "CONTRIBUTING.md": "CONTRIBUTING.md",
    "docs/agent-cli.md": "docs/agent-cli.md",
}


def archive_files():
    files = dict(FILES)
    for path in sorted((ROOT / "specs").rglob("*.md")):
        if path.name == "AGENTS.md":
            continue
        relative = path.relative_to(ROOT).as_posix()
        files[relative] = relative
    return files


def submodule_links(commit):
    config = configparser.ConfigParser()
    config.read(ROOT / ".gitmodules")
    links = {}
    for section in config.sections():
        path = config[section]["path"]
        entry = run("git", "ls-tree", commit, "--", path).split()
        if entry and entry[0] == "160000":
            repository = config[section]["url"].removesuffix(".git")
            links[path] = f"{repository}/tree/{entry[2]}"
    return links


def archive_markdown(source, member, files, commit, submodules):
    """Keep shipped links local and pin source-only links to the candidate commit."""
    text = source.read_text(encoding="utf-8")

    def rewrite(match):
        target = match.group(2)
        parsed = urlsplit(target)
        if parsed.scheme or parsed.netloc or not parsed.path or parsed.path.startswith("/"):
            return match.group(0)
        path = posixpath.normpath(
            posixpath.join(posixpath.dirname(member), unquote(parsed.path))
        )
        if path in files.values():
            return match.group(0)
        kind = "tree" if (ROOT / path).is_dir() else "blob"
        url = submodules.get(path) or (
            f"https://github.com/suraciii/slipstream/{kind}/{commit}/{quote(path, safe='/')}"
        )
        if parsed.query:
            url += f"?{parsed.query}"
        if parsed.fragment:
            url += f"#{parsed.fragment}"
        return f"{match.group(1)}{url}{match.group(3)}"

    return re.sub(r"(\]\()([^\s)]+)(\))", rewrite, text).encode("utf-8")


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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--darktable-image", help="Immutable built photo image ID; also export a dedicated standalone darktable-edit archive")
    args = parser.parse_args(argv or [])
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
        files = archive_files()
        submodules = submodule_links(commit)
        with gzip.GzipFile(fileobj=stream, mode="wb", filename="", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as tar:
                sources = [(binary, "slipstream")]
                sources.extend((ROOT / path, member) for path, member in files.items())
                for source, member in sources:
                    info = file_info(source, f"{name}/{member}")
                    if source.suffix == ".md":
                        data = archive_markdown(source, member, files, commit, submodules)
                        info.size = len(data)
                        tar.addfile(info, io.BytesIO(data))
                    else:
                        with source.open("rb") as data:
                            tar.addfile(info, data)
    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    checksum = output / f"{archive.name}.sha256"
    checksum.write_text(f"{digest.hexdigest()}  {archive.name}\n", encoding="ascii")
    print(output)
    if args.darktable_image:
        helper = ROOT / "tools/processing/photo/package_tool.py"
        spec = importlib.util.spec_from_file_location("package_darktable_tool", helper)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        standalone = dist / f"darktable-edit-{version}-g{commit[:12]}-linux-amd64"
        print(module.package_tool(args.darktable_image, standalone))


if __name__ == "__main__":
    main(sys.argv[1:])
