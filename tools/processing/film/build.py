"""Build the application image with the pinned spektrafilm-rs extension.

The application runtime remains the immutable parent image. The extension
adds the fork CLI, its profile data, a module-owned default recipe, and the
deterministic bundle manifest. Darktable is a separate peer engine.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

APP_TARGET = "runtime"
DEFAULT_APP_TAG = "slipstream:app-runtime"
FILM_DOCKERFILE = "tools/processing/film/Dockerfile"
SERVER_ENTRYPOINT = ["/usr/local/bin/slipstream-server"]
ROOT = Path(__file__).resolve().parents[3]
PINNED_SPEKTRAFILM_FORK_COMMIT = "5c6958a33fc5f2a41b31e724146334d59a950f0e"


def inspect(reference: str) -> dict:
    return json.loads(subprocess.check_output(["docker", "image", "inspect", reference], text=True))[0]


def repository_revision() -> str:
    return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()
def spektrafilm_fork_revision() -> str:
    revision = subprocess.check_output(
        ["git", "-C", str(ROOT / "third_party/spektrafilm-rs"), "rev-parse", "HEAD"],
        text=True,
    ).strip()
    if revision != PINNED_SPEKTRAFILM_FORK_COMMIT:
        raise SystemExit(
            f"third_party/spektrafilm-rs is at {revision}, expected {PINNED_SPEKTRAFILM_FORK_COMMIT}"
        )
    return revision


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


def build_film(app_reference: str, tag: str, bundle: str) -> None:
    subprocess.run(
        [
            "docker", "build", "--pull=false", "--progress=plain",
            "--build-arg", f"APP_IMAGE={app_reference}",
            "--build-arg", f"SPEKTRAFILM_FORK_COMMIT={spektrafilm_fork_revision()}",
            "--build-arg", f"FILM_BUNDLE={bundle}",
            "-f", FILM_DOCKERFILE, "-t", tag, ".",
        ], cwd=ROOT, env=buildkit_environment(), check=True,
    )


def read_bundle(tag: str) -> str:
    bundle = subprocess.check_output(
        ["docker", "run", "--rm", "--entrypoint", "cat", tag, "/opt/slipstream-film/bundle"], text=True
    ).strip()
    if len(bundle) != 64 or any(c not in "0123456789abcdef" for c in bundle):
        raise SystemExit("manifest helper produced an invalid bundle digest")
    return bundle


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
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
    if args.app_image and args.app_tag != DEFAULT_APP_TAG:
        raise SystemExit("pass either --app-image or --app-tag, not both")
    if args.tag == args.app_tag:
        raise SystemExit("the output tag must not overwrite the application runtime tag")

    app_reference = args.app_image or args.app_tag
    if not args.app_image:
        build_app(args.app_tag)
    app = inspect(app_reference)

    build_film(app_reference, args.tag, "unresolved")
    bundle = read_bundle(args.tag)
    build_film(app_reference, args.tag, bundle)
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
            "spektrafilm_fork_commit": spektrafilm_fork_revision(),
        },
        sort_keys=True,
    ))


if __name__ == "__main__":
    main()
