"""Executed inside the isolated qualification container, never in the service."""

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import resource
import shutil
import sqlite3
import struct
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

import numpy as np
import OpenImageIO as oiio
import tifffile

from film import make_simulator, pixel_digest, render, reset_random_state

WORK = Path("/work")
RAW = Path("/input") / os.environ["PROBE_RAW_NAME"]
DT = "http://darktable.sf.net/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
ICC = Path("/opt/spektrafilm/src/spektrafilm/data/icc/ellelstone/LargeRGB-elle-V2-g10.icc")
ICC_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"
# Reading the description through LittleCMS parses and reserializes its legacy
# desc tag. This pinned output differs only in that tag's representation.
OUTPUT_ICC_SHA256 = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
VERSIONS = {
    "rawprepare": 2, "demosaic": 6, "colorin": 7, "colorout": 5,
    "gamma": 1, "temperature": 4, "highlights": 4, "flip": 2,
}


def emit(event, **facts):
    print(json.dumps({"event": event, **facts}, allow_nan=False), flush=True)


def read_image(path):
    reader = oiio.ImageInput.open(str(path))
    if reader is None:
        raise RuntimeError(oiio.geterror())
    try:
        result = reader.read_image()
        if result is None:
            raise RuntimeError(reader.geterror())
        return result
    finally:
        reader.close()


def inspect_tiff(path, expected_icc=OUTPUT_ICC_SHA256):
    with tifffile.TiffFile(path) as image:
        page = image.pages[0]
        assert page.dtype == np.dtype("float32") and page.samplesperpixel == 3
        assert page.photometric == 2 and page.sampleformat == 3
        assert page.compression == 8
        assert page.tags.get(274) is None or page.tags[274].value == 1
        icc = page.tags[34675].value
        assert hashlib.sha256(icc).hexdigest() == expected_icc
        assert icc[16:20] == b"RGB " and icc[20:24] == b"XYZ "
        tags = {}
        for index in range(struct.unpack_from(">I", icc, 128)[0]):
            offset = 132 + index * 12
            name = icc[offset:offset + 4].decode("ascii")
            start, size = struct.unpack_from(">II", icc, offset + 4)
            assert start + size <= len(icc)
            tags[name] = icc[start:start + size]
        for name in ("rTRC", "gTRC", "bTRC"):
            curve = tags[name]
            assert curve[:4] == b"curv"
            assert struct.unpack_from(">I", curve, 8)[0] == 1
            assert struct.unpack_from(">H", curve, 12)[0] == 256
        # ICC XYZ tags use signed 16.16 values. The xy comparison allows four
        # encoding units because normalization propagates all three errors.
        # The engine's profile uses its own D50 and primary constants. Record
        # chromaticities for independent qualification; do not infer primaries
        # from a human-readable profile name.
        xyz = {}
        for name in ("wtpt", "rXYZ", "gXYZ", "bXYZ"):
            assert tags[name][:4] == b"XYZ "
            xyz[name] = [v / 65536 for v in struct.unpack_from(">iii", tags[name], 8)]
        primaries = [[xyz[k][0] / sum(xyz[k]), xyz[k][1] / sum(xyz[k])]
                     for k in ("rXYZ", "gXYZ", "bXYZ")]
        np.testing.assert_allclose(
            primaries, [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]],
            rtol=0, atol=4 / 65536,
        )
        white = xyz["wtpt"]
        np.testing.assert_allclose(
            [white[0] / sum(white), white[1] / sum(white)],
            [0.3457, 0.3585], rtol=0, atol=4 / 65536,
        )
        return {
            "shape": list(page.shape), "sample_format": "float32",
            "icc_sha256": hashlib.sha256(icc).hexdigest(), "icc_xyz": xyz,
            "bytes": path.stat().st_size,
        }


def darktable(output, *, xmp=None, full=False, database="reference.db"):
    args = ["darktable-cli", str(RAW)]
    if xmp is not None:
        args.append(str(xmp))
    args += [str(output), "--width", "0" if full else "1224",
             "--height", "0" if full else "1224", "--hq", "true",
             "--apply-custom-presets", "false", "--icc-type", "FILE",
             "--icc-file", "/work/config/color/out/linear-prophoto.icc",
             "--core", "--disable-opencl", "--configdir", "/work/config",
             "--cachedir", "/work/cache", "--tmpdir", "/work/tmp",
             "--library", str(WORK / database),
             "--conf", "plugins/darkroom/workflow=none",
             "--conf", "plugins/imageio/format/tiff/bpp=32",
             "--conf", "plugins/imageio/format/tiff/compress=1",
             "--conf", "plugins/imageio/format/tiff/compresslevel=6"]
    start = time.monotonic()
    child = subprocess.run(args, capture_output=True, text=True, check=False)
    (WORK / (output.stem + ".log")).write_text(child.stdout + child.stderr)
    if child.returncode != 0:
        raise RuntimeError(f"darktable failed ({child.returncode}); see {output.stem}.log")
    emit("darktable", output=output.name, seconds=time.monotonic() - start,
         full_resolution=full, **inspect_tiff(output))


def generated_history(exposure):
    """Probe the pinned private format and engine serialization behavior."""
    with sqlite3.connect(f"file:{WORK / 'reference.db'}?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        rows = list(db.execute("SELECT * FROM history WHERE imgid=1 ORDER BY num"))
        assert {r["operation"]: r["module"] for r in rows} == VERSIONS
        order = db.execute("SELECT version FROM module_order WHERE imgid=1").fetchone()[0]
        assert order == 4
    for prefix, uri in [("x", "adobe:ns:meta/"), ("rdf", RDF), ("darktable", DT)]:
        ET.register_namespace(prefix, uri)
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
    values = {
        "num": str(len(rows)), "operation": "exposure", "enabled": "1",
        "modversion": "7", "params": struct.pack("<iffffii", 0, 0, exposure, 50, -4, 0, 0).hex(),
        "multi_priority": "0", "multi_name": "", "multi_name_hand_edited": "0",
    }
    ET.SubElement(sequence, f"{{{RDF}}}li", {f"{{{DT}}}{k}": v for k, v in values.items()})
    output = WORK / f"exposure-{exposure}.xmp"
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)
    return output


def raw_probe(full):
    source_digest = hashlib.file_digest(RAW.open("rb"), "sha256").hexdigest()
    darktable(WORK / "baseline.tif")
    baseline = read_image(WORK / "baseline.tif")
    from spektrafilm.utils.io import load_image_oiio, save_image_oiio
    np.testing.assert_array_equal(baseline, load_image_oiio(str(WORK / "baseline.tif")))
    sentinel = np.tile(np.array([-0.25, 0.0, 0.18, 1.0, 1.25, 4.0], dtype=np.float32), 32)
    sentinel = sentinel.reshape(8, 8, 3)
    interchange = WORK / "interchange.tif"
    save_image_oiio(str(interchange), sentinel, bit_depth=32,
                   color_space="ProPhoto RGB", cctf_encoding=False)
    inspect_tiff(interchange, ICC_SHA256)
    np.testing.assert_array_equal(sentinel, load_image_oiio(str(interchange)))
    emit("tiff_interchange", compressed_roundtrip_exact=True,
         negative_and_over_range_preserved=True, spektrafilm_reader_exact=True)
    # Generate both before a later run can change a reference database.
    histories = [generated_history(ev) for ev in (0, 1)]
    for ev, history in enumerate(histories):
        darktable(WORK / f"exposure-{ev}.tif", xmp=history, database=f"ev-{ev}.db")
    zero = read_image(WORK / "exposure-0.tif")
    one = read_image(WORK / "exposure-1.tif")
    np.testing.assert_array_equal(zero, baseline)
    with sqlite3.connect(f"file:{WORK / 'ev-1.db'}?mode=ro", uri=True) as db:
        row = db.execute("SELECT op_params,enabled,module FROM history WHERE operation='exposure'").fetchone()
        assert row == (struct.pack("<iffffii", 0, 0, 1, 50, -4, 0, 0), 1, 7)
    # A fresh CLI reads the saved database without the generated XMP carrier.
    darktable(WORK / "reference-ev-1.tif", database="ev-1.db")
    np.testing.assert_array_equal(one, read_image(WORK / "reference-ev-1.tif"))
    emit("exposure", baseline_exact=True, database_reload_exact=True,
         doubling_residual=float(np.max(np.abs(one - 2 * zero))),
         note="Whole-pipeline exact doubling is not asserted: the pinned Lab conversion approximates cube roots.")
    if full:
        darktable(WORK / "development.tif", full=True, xmp=histories[0], database="full.db")
        pixels = read_image(WORK / "development.tif")
        assert np.isfinite(pixels).all(), "Development TIFF contains non-finite samples"
        emit("full_tiff_samples", minimum=float(pixels.min()), maximum=float(pixels.max()),
             finite=bool(np.isfinite(pixels).all()))
    assert hashlib.file_digest(RAW.open("rb"), "sha256").hexdigest() == source_digest


def synthetic(height, width):
    pixels = np.empty((height, width, 3), dtype=np.float64)
    pixels[:, :, 0] = np.linspace(0.002, 1.4, width)[None, :]
    pixels[:, :, 1] = np.linspace(0.05, 1.0, height)[:, None]
    pixels[:, :, 2] = 0.18
    return pixels


def film_probe(mode, repetitions):
    start = time.monotonic()
    simulator, recipe = make_simulator()
    (WORK / "film-recipe.json").write_text(json.dumps(recipe, indent=2))
    emit("film_initialize", seconds=time.monotonic() - start)
    sizes = [(128, 192)] if mode == "smoke" else [(816, 1224), (1152, 1728)]
    for height, width in sizes:
        pixels = synthetic(height, width)
        expected = None
        times = []
        for index in range(repetitions + 1):
            start = time.monotonic()
            output = render(simulator, pixels)
            elapsed = time.monotonic() - start
            digest = pixel_digest(output)
            if expected is None:
                expected = digest
            assert digest == expected, "Repeated Film samples changed"
            if index:
                times.append(elapsed)
            emit("film_render", geometry=[height, width], iteration=index,
                 seconds=elapsed, pixel_sha256=digest,
                 peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
        # A different exposure must not contaminate the next render of A.
        render(simulator, pixels * 2)
        assert pixel_digest(render(simulator, pixels)) == expected
        emit("film_distribution", geometry=[height, width], warm_samples=len(times),
             p50_seconds=float(np.median(times)),
             p95_seconds=sorted(times)[math.ceil(0.95 * len(times)) - 1],
             aba_exact=True)
    from spektrafilm.model.grain import add_micro_structure
    reset_random_state()
    first = add_micro_structure(np.ones((64, 64, 3)), (0.1, 30), 0.3)
    reset_random_state()
    second = add_micro_structure(np.ones((64, 64, 3)), (0.1, 30), 0.3)
    np.testing.assert_array_equal(first, second)
    assert first.std() > 0
    emit("microstructure", repeatable=True, standard_deviation=float(first.std()))
    if mode == "full":
        pixels = read_image(WORK / "development.tif")
        start = time.monotonic()
        output = render(simulator, pixels)
        emit("full_film", seconds=time.monotonic() - start, geometry=list(output.shape),
             pixel_sha256=pixel_digest(output), timings=simulator.get_timings(),
             peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
        from spektrafilm.utils.io import save_image_oiio
        save_image_oiio(str(WORK / "finished.jpg"), output, color_space="sRGB", cctf_encoding=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", choices=("raw", "pipeline"), default="pipeline")
    parser.add_argument("--mode", choices=("smoke", "benchmark", "full"), default="smoke")
    parser.add_argument("--repetitions", type=int, default=20)
    args = parser.parse_args()
    if args.repetitions < 2:
        parser.error("at least two warm repetitions are required")
    assert not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY")
    assert not Path("/dev/dri").exists() and not list(Path("/dev").glob("nvidia*"))
    for name in ("config", "cache", "tmp", "xdgconfig/darktable", "matplotlib"):
        (WORK / name).mkdir(parents=True, exist_ok=True)
    assert hashlib.sha256(ICC.read_bytes()).hexdigest() == ICC_SHA256
    (WORK / "config/color/out").mkdir(parents=True)
    shutil.copyfile(ICC, WORK / "config/color/out/linear-prophoto.icc")
    oiio.attribute("threads", 4)
    emit("runtime", python=sys.version.split()[0],
         packages={d.metadata["Name"]: d.version for d in importlib.metadata.distributions()},
         darktable=subprocess.check_output(["darktable-cli", "--version"], text=True).splitlines()[0])
    raw_probe(args.mode == "full")
    if args.stage == "pipeline":
        film_probe(args.mode, args.repetitions)
    assert not any(name.startswith(("napari", "PySide", "PyQt")) for name in sys.modules)
    emit("probe_complete", qualification="incomplete",
         remaining=["camera WB mapping corpus", "independent EV numerical oracle",
                    "LUT accuracy versus direct spectral reference", "fresh-process repeats",
                    "browse contention and resource defaults", "product latency decision"])


if __name__ == "__main__":
    main()
