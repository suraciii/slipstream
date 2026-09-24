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
import struct
import subprocess
import sys
import time

import numpy as np
import OpenImageIO as oiio
import tifffile

from film import make_simulator, pixel_digest, render, reset_random_state
from bundle import load_bundle
from history import generated_history, validate_imported_history

WORK = Path("/work")
RAW = Path("/input") / os.environ["PROBE_RAW_NAME"]
ICC = Path("/opt/spektrafilm/src/spektrafilm/data/icc/ellelstone/LargeRGB-elle-V2-g10.icc")
ICC_SHA256 = "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed"
# Reading the description through LittleCMS parses and reserializes its legacy
# desc tag. This pinned output differs only in that tag's representation.
OUTPUT_ICC_SHA256 = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
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
    # Keep this history probe separate from any independent authoring reference.
    cases = [(0.0, False), (1.0, False), (1.0, True)]
    histories = []
    for ev, custom_wb in cases:
        suffix = f"ev-{ev:g}" + ("-custom-wb" if custom_wb else "")
        history = generated_history(
            WORK / "reference.db", WORK / f"{suffix}.xmp", ev, custom_wb=custom_wb
        )
        histories.append(history)
        darktable(WORK / f"{suffix}.tif", xmp=history, database=f"{suffix}.db")
        facts = validate_imported_history(
            WORK / f"{suffix}.db", ev, custom_wb=custom_wb
        )
        emit("raw_edit_history", **facts)
    zero = read_image(WORK / "ev-0.tif")
    one = read_image(WORK / "ev-1.tif")
    one_custom_wb = read_image(WORK / "ev-1-custom-wb.tif")
    np.testing.assert_array_equal(zero, baseline)
    assert not np.array_equal(one_custom_wb, one), "Custom white balance did not alter the output"
    # A fresh CLI reads the saved custom history without the generated XMP carrier.
    darktable(WORK / "history-roundtrip.tif", database="ev-1-custom-wb.db")
    np.testing.assert_array_equal(one_custom_wb, read_image(WORK / "history-roundtrip.tif"))
    emit("exposure_white_balance", baseline_exact=True, database_reload_exact=True,
         doubling_residual=float(np.max(np.abs(one - 2 * zero))),
         custom_wb_changed_output=True,
         note="This is a generated-history check, not an independently authored reference. Whole-pipeline exact doubling is not asserted because the pinned Lab conversion approximates cube roots.")
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
    emit("runtime", processing_bundle=load_bundle(), python=sys.version.split()[0],
         packages={d.metadata["Name"]: d.version for d in importlib.metadata.distributions()},
         darktable=subprocess.check_output(["darktable-cli", "--version"], text=True).splitlines()[0])
    raw_probe(args.mode == "full")
    if args.stage == "pipeline":
        film_probe(args.mode, args.repetitions)
    assert not any(name.startswith(("napari", "PySide", "PyQt")) for name in sys.modules)
    emit("probe_complete", qualification="incomplete",
         remaining=["independently authored nonzero-EV/custom-WB reference",
                    "camera WB mapping corpus and temperature/tint mapping",
                    "LUT accuracy versus direct spectral reference", "fresh-process repeats",
                    "browse contention and resource defaults", "product latency decision"])


if __name__ == "__main__":
    main()
