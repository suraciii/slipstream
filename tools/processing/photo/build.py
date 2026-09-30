"""Build the photo worker against a locally checked-out native darktable commit."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

PARENT = "sha256:10aa79ce1148ba8aec83f6f68e6d39ba7edaa88f2369f21fac65d4784f4b495d"
PARENT_TAG = "slipstream:344-buffer-lifetimes"
ROOT = Path(__file__).resolve().parents[3]


def inspect(reference: str) -> dict:
    return json.loads(subprocess.check_output(["docker", "image", "inspect", reference], text=True))[0]


def _submodule_paths(source: Path) -> list[Path]:
    status = subprocess.check_output(
        ["git", "-C", str(source), "submodule", "status", "--recursive"], text=True,
    )
    paths = [source]
    for line in status.splitlines():
        fields = line[1:].split(maxsplit=1)
        if len(fields) == 2:
            path = source / fields[1].split(" (", 1)[0]
            if path.is_dir():
                paths.append(path)
    return paths


def _context_changes(repo: Path) -> list[str]:
    output = subprocess.check_output(
        [
            "git", "-C", str(repo), "status", "--porcelain=v1", "-z",
            "--untracked-files=all", "--ignored",
        ],
    )
    changes = []
    for record in output.decode().split("\0"):
        if not record:
            continue
        path = record[3:]
        if any(part in {".git", "docker-images"} for part in Path(path).parts):
            continue
        changes.append(f"{repo}: {record}")
    return changes


def git_revision(source: Path) -> str:
    changes = [
        change
        for repo in _submodule_paths(source)
        for change in _context_changes(repo)
    ]
    if changes:
        raise SystemExit(
            "native source context has uncommitted or untracked files; "
            "build only from the exact committed source: " + "; ".join(changes)
        )
    status = subprocess.check_output(
        ["git", "-C", str(source), "submodule", "status", "--recursive"], text=True,
    )
    if any(line[:1] in {"+", "-", "U"} for line in status.splitlines()):
        raise SystemExit("native source has uncommitted or unresolved submodules")
    return subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()


def build(source: Path, commit: str, tag: str, bundle: str) -> None:
    environment = os.environ.copy()
    environment["DOCKER_BUILDKIT"] = "1"
    subprocess.run(
        [
            "docker", "build", "--pull=false", "--progress=plain",
            "--build-context", f"darktable={source}",
            "--build-arg", f"DARKTABLE_COMMIT={commit}",
            "--build-arg", f"PHOTO_BUNDLE={bundle}",
            "-f", "tools/processing/photo/Dockerfile", "-t", tag, ".",
        ], cwd=ROOT, env=environment, check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--darktable-commit", required=True, help="full 40-character native source revision")
    parser.add_argument("--darktable-source", type=Path, required=True, help="checked-out native darktable source")
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    source = args.darktable_source.resolve()
    if args.tag in (PARENT, PARENT_TAG):
        raise SystemExit("the output tag must not overwrite the numerical parent")
    if len(args.darktable_commit) != 40 or any(c not in "0123456789abcdef" for c in args.darktable_commit):
        raise SystemExit("--darktable-commit must be a lowercase full commit hash")
    if git_revision(source) != args.darktable_commit:
        raise SystemExit("native source HEAD does not match --darktable-commit")
    parent = inspect(PARENT_TAG)
    if parent["Id"] != PARENT:
        raise SystemExit("the retained numerical parent image is absent or has changed")

    build(source, args.darktable_commit, args.tag, "unresolved")
    bundle = subprocess.check_output(
        ["docker", "run", "--rm", "--entrypoint", "cat", args.tag, "/opt/slipstream-photo/bundle"], text=True
    ).strip()
    if len(bundle) != 64 or any(c not in "0123456789abcdef" for c in bundle):
        raise SystemExit("manifest helper produced an invalid bundle digest")
    build(source, args.darktable_commit, args.tag, bundle)
    output = inspect(args.tag)
    current = inspect(PARENT_TAG)
    layers = parent["RootFS"]["Layers"]
    if current["Id"] != PARENT or output["RootFS"]["Layers"][:len(layers)] != layers:
        raise SystemExit("build did not preserve the qualified numerical parent layers")
    print(json.dumps({"parent": PARENT, "image": output["Id"], "bundle": bundle, "darktable_commit": args.darktable_commit}, sort_keys=True))


if __name__ == "__main__":
    main()
