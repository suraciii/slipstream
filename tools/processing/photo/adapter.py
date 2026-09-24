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
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

DT = "http://darktable.sf.net/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

# The linear ProPhoto RGB handoff profile lives at <work>/config/color/out;
# darktable resolves --icc-file against profiles registered under the
# configdir the worker prepares there.
PROFILE_RELATIVE = Path("config/color/out/linear-prophoto.icc")
ICC_ASSET_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"

BASELINE_EDGE = "1224"
EXPECTED_REFUSED = 71

VERSIONS = {
    "rawprepare": 2,
    "demosaic": 6,
    "colorin": 7,
    "colorout": 5,
    "gamma": 1,
    "temperature": 4,
    "highlights": 4,
    "flip": 2,
}


def exposure_params(exposure_ev):
    # mode=manual, black=0, EV, deflicker defaults, both bias adjustments off
    if exposure_ev < 0.0 or exposure_ev > 1.0 or not (exposure_ev == exposure_ev):
        raise ValueError("exposure outside the protocol range")
    return struct.pack("<iffffii", 0, 0.0, exposure_ev, 50.0, -4.0, 0, 0)


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


def baseline_rows(work):
    database = work / "reference.db"
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        rows = list(db.execute("SELECT * FROM history WHERE imgid=1 ORDER BY num"))
        order = db.execute("SELECT version FROM module_order WHERE imgid=1").fetchone()
    operations = Counter(row["operation"] for row in rows)
    if operations != Counter({operation: 1 for operation in VERSIONS}):
        raise RuntimeError(
            "Unexpected darktable baseline history; refusing implicit or duplicate modules: "
            f"{dict(operations)}"
        )
    if order is None or order[0] != 4:
        raise RuntimeError("Unsupported darktable module-order version")
    if any(row["module"] != VERSIONS[row["operation"]] for row in rows):
        raise RuntimeError("Unsupported darktable baseline module version")
    return rows


def generated_history(rows, output, exposure_ev):
    """Serialize baseline rows plus the manual exposure module as XMP."""
    ET.register_namespace("x", "adobe:ns:meta/")
    ET.register_namespace("rdf", RDF)
    ET.register_namespace("darktable", DT)
    root = ET.Element("{adobe:ns:meta/}xmpmeta")
    rdf = ET.SubElement(root, f"{{{RDF}}}RDF")
    desc = ET.SubElement(rdf, f"{{{RDF}}}Description", {
        f"{{{RDF}}}about": "", f"{{{DT}}}xmp_version": "5",
        f"{{{DT}}}history_end": str(len(rows) + 1),
        f"{{{DT}}}iop_order_version": "4", f"{{{DT}}}auto_presets_applied": "1",
    })
    sequence = ET.SubElement(ET.SubElement(desc, f"{{{DT}}}history"), f"{{{RDF}}}Seq")
    mapping = {
        "num": "num", "operation": "operation", "enabled": "enabled",
        "modversion": "module", "params": "op_params",
        "blendop_params": "blendop_params", "blendop_version": "blendop_version",
        "multi_priority": "multi_priority", "multi_name": "multi_name",
        "multi_name_hand_edited": "multi_name_hand_edited",
    }
    for row in rows:
        values = {key: row[column] for key, column in mapping.items()}
        ET.SubElement(sequence, f"{{{RDF}}}li", {
            f"{{{DT}}}{key}": value.hex() if isinstance(value, bytes) else str(value)
            for key, value in values.items() if value is not None
        })
    appended = {
        "num": str(len(rows)), "operation": "exposure", "enabled": "1",
        "modversion": "7", "params": exposure_params(exposure_ev).hex(),
        "multi_priority": "0", "multi_name": "", "multi_name_hand_edited": "0",
    }
    ET.SubElement(sequence, f"{{{RDF}}}li", {f"{{{DT}}}{k}": v for k, v in appended.items()})
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)
    return output


def validate_imported_history(work, exposure_ev):
    database = work / "library.db"
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as db:
        rows = list(db.execute(
            "SELECT num, operation, enabled, module, op_params "
            "FROM history WHERE imgid=1 ORDER BY num"
        ))
    expected_order = (*VERSIONS, "exposure")
    operations = Counter(row[1] for row in rows)
    if operations != Counter({operation: 1 for operation in expected_order}):
        raise RuntimeError(
            "Unexpected imported darktable history; duplicate adaptation or automatic/look module: "
            f"{dict(operations)}"
        )
    numbers = [row[0] for row in rows]
    if numbers != list(range(len(expected_order))) or [row[1] for row in rows] != list(expected_order):
        raise RuntimeError(
            "Unexpected imported darktable history sequence or numbering: "
            f"num={numbers}, operations={[row[1] for row in rows]}"
        )
    by_operation = {row[1]: row for row in rows}
    temperature = by_operation["temperature"]
    if temperature[3] != VERSIONS["temperature"] or not temperature[2]:
        raise RuntimeError("darktable white-balance module is disabled or has an unknown version")
    exposure = by_operation["exposure"]
    if exposure[2:] != (1, 7, exposure_params(exposure_ev)):
        raise RuntimeError("darktable did not import the requested manual exposure parameters")


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
    try:
        run_darktable(args.input, work / "baseline.tif", None, work, BASELINE_EDGE, "reference.db")
        rows = baseline_rows(work)
        history = generated_history(rows, work / "development.xmp", exposure_ev)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        run_darktable(args.input, args.output, history, work, "0", "library.db")
        validate_imported_history(work, exposure_ev)

        if not args.output.is_file() or args.output.stat().st_size == 0:
            raise RuntimeError("darktable produced no output")
    except (RuntimeError, sqlite3.Error, OSError) as error:
        print(f"engine stage failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
