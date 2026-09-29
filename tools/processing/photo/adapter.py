"""Pinned development-tiff engine adapter (production worker side).

Reproduces the qualified darktable history pipeline from
tools/development (probe.py + history.py) using only the standard
library: a bounded baseline run establishes the default module history,
the as-shot exposure adaptation is appended through a generated XMP
sidecar, the final full-size run imports that history, and the imported
database is validated before the produced TIFF is accepted.

Refuses to run darktable at all unless the container configdir carries
the exact linear ProPhoto RGB output profile: a missing or altered
profile would otherwise be silently replaced by sRGB.
"""

import argparse
import hashlib
import sqlite3
import subprocess
import sys
from pathlib import Path

try:
    from history import exposure_params, generated_history, validate_imported_history
except ModuleNotFoundError:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "development"))
    from history import exposure_params, generated_history, validate_imported_history


# The linear ProPhoto RGB handoff profile lives at <work>/config/color/out;
# darktable resolves --icc-file against profiles registered under the
# configdir the worker prepares there.
PROFILE_RELATIVE = Path("config/color/out/linear-prophoto.icc")
ICC_ASSET_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"

BASELINE_EDGE = "1224"
EXPECTED_REFUSED = 71

def run_darktable(input_path, output, xmp, work, edge, database):
    args = ["darktable-cli", str(input_path)]
    if xmp is not None:
        args.append(str(xmp))
    args += [
        str(output), "--width", edge, "--height", edge, "--hq", "true",
        "--apply-custom-presets", "false", "--icc-type", "FILE",
        "--icc-file", str(work / PROFILE_RELATIVE),
        "--core", "--disable-opencl", "--configdir", str(work / "config"),
        "--cachedir", str(work / "cache"), "--tmpdir", str(work / "tmp"),
        "--library", str(work / database),
        "--conf", "plugins/darkroom/workflow=none",
        "--conf", "plugins/imageio/format/tiff/bpp=32",
        "--conf", "plugins/imageio/format/tiff/compress=1",
        "--conf", "plugins/imageio/format/tiff/compresslevel=6",
    ]
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    (work / (Path(output).stem + ".log")).write_text(result.stdout + result.stderr)
    if result.returncode != 0 or not Path(output).is_file():
        raise RuntimeError(f"darktable failed with exit {result.returncode}")



def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as reader:
        for chunk in iter(lambda: reader.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--exposure-milli-ev", type=int, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--icc-asset", type=Path, required=True)
    args = parser.parse_args()

    # Fail closed before any engine run: without the exact handoff profile
    # darktable silently exports sRGB instead of linear ProPhoto RGB.
    for profile in (args.work / PROFILE_RELATIVE, args.icc_asset):
        if not profile.is_file() or sha256_file(profile) != ICC_ASSET_SHA256:
            print(f"refusing to develop without the pinned output profile: {profile}", file=sys.stderr)
            return EXPECTED_REFUSED

    work = args.work
    (work / "config/color/out").mkdir(parents=True, exist_ok=True)
    (work / "cache").mkdir(exist_ok=True)
    (work / "tmp").mkdir(exist_ok=True)
    exposure_ev = args.exposure_milli_ev / 1000.0
    if exposure_ev < 0.0 or exposure_ev > 1.0:
        raise ValueError("exposure outside the protocol range")
    try:
        run_darktable(args.input, work / "baseline.tif", None, work, BASELINE_EDGE, "reference.db")
        history = generated_history(work / "reference.db", work / "development.xmp", exposure_ev)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        run_darktable(args.input, args.output, history, work, "0", "library.db")
        validate_imported_history(work / "library.db", exposure_ev)

        if not args.output.is_file() or args.output.stat().st_size == 0:
            raise RuntimeError("darktable produced no output")
    except (RuntimeError, sqlite3.Error, OSError) as error:
        print(f"engine stage failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
