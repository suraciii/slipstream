"""Build the application image with the native darktable extension attached.

The normal application runtime from the repository Dockerfile is built (or an
already-built immutable application image is supplied with ``--app-image``)
and then extended by ``tools/processing/photo/Dockerfile`` with the pinned
native engine, its discovered MCP metadata, the ICC output profile, and the
deterministic bundle manifest. The extended image keeps the Slipstream server
entrypoint: it is the same application, with the optional Photo Development
extension installed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

APP_TARGET = "runtime"
DEFAULT_APP_TAG = "slipstream:app-runtime"
PHOTO_DOCKERFILE = "tools/processing/photo/Dockerfile"
PHOTO_ICC = "tools/processing/photo/icc/LargeRGB-elle-V2-g10.icc"
# The qualified Development output profile (also pinned by qualify.py and the
# acceptance runner's embedded-profile digests).
PINNED_ICC_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"
SERVER_ENTRYPOINT = ["/usr/local/bin/slipstream-server"]
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


def file_digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def repository_revision() -> str:
    return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()


def verify_pinned_icc() -> None:
    path = ROOT / PHOTO_ICC
    if not path.is_file():
        raise SystemExit(f"the qualified ICC profile is missing from the repository: {PHOTO_ICC}")
    observed = file_digest(path)
    if observed != PINNED_ICC_SHA256:
        raise SystemExit(
            f"the vendored ICC profile digest {observed} does not match the qualified profile {PINNED_ICC_SHA256}"
        )


def buildkit_environment() -> dict:
    environment = os.environ.copy()
    environment["DOCKER_BUILDKIT"] = "1"
    return environment


def build_app(tag: str) -> None:
    subprocess.run(
        [
            "docker", "build", "--pull=false", "--progress=plain",
            "--target", APP_TARGET,
            "--build-arg", f"SLIPSTREAM_VCS_REF={repository_revision()}",
            "-f", "Dockerfile", "-t", tag, ".",
        ], cwd=ROOT, env=buildkit_environment(), check=True,
    )


def build_photo(app_reference: str, source: Path, commit: str, tag: str, bundle: str) -> None:
    subprocess.run(
        [
            "docker", "build", "--pull=false", "--progress=plain",
            "--build-context", f"darktable={source}",
            "--build-arg", f"APP_IMAGE={app_reference}",
            "--build-arg", f"DARKTABLE_COMMIT={commit}",
            "--build-arg", f"PHOTO_BUNDLE={bundle}",
            "-f", PHOTO_DOCKERFILE, "-t", tag, ".",
        ], cwd=ROOT, env=buildkit_environment(), check=True,
    )


def read_bundle(tag: str) -> str:
    bundle = subprocess.check_output(
        ["docker", "run", "--rm", "--entrypoint", "cat", tag, "/opt/slipstream-photo/bundle"], text=True
    ).strip()
    if len(bundle) != 64 or any(c not in "0123456789abcdef" for c in bundle):
        raise SystemExit("manifest helper produced an invalid bundle digest")
    return bundle


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--darktable-commit", required=True, help="full 40-character native source revision")
    parser.add_argument("--darktable-source", type=Path, required=True, help="checked-out native darktable source")
    parser.add_argument("--tag", required=True, help="tag for the extended application image")
    parser.add_argument(
        "--app-image",
        help="already-built immutable application image reference; skips the application build",
    )
    parser.add_argument(
        "--app-tag",
        default=DEFAULT_APP_TAG,
        help=f"tag for the freshly built application runtime (default {DEFAULT_APP_TAG})",
    )
    args = parser.parse_args()
    source = args.darktable_source.resolve()
    if args.app_image and args.app_tag != DEFAULT_APP_TAG:
        raise SystemExit("pass either --app-image or --app-tag, not both")
    if args.tag == args.app_tag:
        raise SystemExit("the output tag must not overwrite the application runtime tag")
    if len(args.darktable_commit) != 40 or any(c not in "0123456789abcdef" for c in args.darktable_commit):
        raise SystemExit("--darktable-commit must be a lowercase full commit hash")
    if git_revision(source) != args.darktable_commit:
        raise SystemExit("native source HEAD does not match --darktable-commit")
    verify_pinned_icc()

    app_reference = args.app_image or args.app_tag
    if not args.app_image:
        build_app(args.app_tag)
    app = inspect(app_reference)

    build_photo(app_reference, source, args.darktable_commit, args.tag, "unresolved")
    bundle = read_bundle(args.tag)
    build_photo(app_reference, source, args.darktable_commit, args.tag, bundle)
    output = inspect(args.tag)
    current = inspect(app_reference)
    layers = app["RootFS"]["Layers"]
    if current["Id"] != app["Id"] or output["RootFS"]["Layers"][:len(layers)] != layers:
        raise SystemExit("build did not extend the exact application runtime layers")
    if output["Config"].get("Entrypoint") != SERVER_ENTRYPOINT:
        raise SystemExit("the extended image must keep the Slipstream server entrypoint")
    print(json.dumps(
        {
            "app": app["Id"],
            "image": output["Id"],
            "bundle": bundle,
            "darktable_commit": args.darktable_commit,
        },
        sort_keys=True,
    ))


if __name__ == "__main__":
    main()
