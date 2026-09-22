"""Build against the retained exact numerical image, without registry substitution."""

import argparse
import json
from pathlib import Path
import subprocess

PARENT = "sha256:b5b140a7646139d654734801c3b4289d6098b626002c6b2732c47573df206972"
PARENT_TAG = "slipstream:358-pre-grain-reclaim"
ROOT = Path(__file__).resolve().parents[3]


def inspect(reference):
    return json.loads(subprocess.check_output(["docker", "image", "inspect", reference]))[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=("qualification", "adapter-checks", "adapter-source"), default="qualification")
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    if args.tag in (PARENT, PARENT_TAG):
        raise SystemExit("The output tag must not overwrite the numerical parent")
    original = inspect(PARENT_TAG)
    if original["Id"] != PARENT:
        raise SystemExit("The retained numerical parent image is absent or has changed")
    subprocess.run(["docker", "build", "--pull=false", "-f", "tools/processing/film/Dockerfile",
                    "--target", args.target, "-t", args.tag, "."], cwd=ROOT, check=True)
    current = inspect(PARENT_TAG)
    output = inspect(args.tag)
    layers = original["RootFS"]["Layers"]
    if current["Id"] != PARENT or output["RootFS"]["Layers"][:len(layers)] != layers:
        raise SystemExit("Build did not preserve the qualified numerical parent layers")
    print(json.dumps({"parent": PARENT, "image": output["Id"], "target": args.target}, sort_keys=True))


if __name__ == "__main__":
    main()
