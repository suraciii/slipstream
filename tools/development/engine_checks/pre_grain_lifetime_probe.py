"""Actual cold interpolation ownership; no saved frames or test-side collection."""

import gc
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from unittest.mock import patch
import weakref

# A private directory is established before importing any numerical code.
os.environ["NUMBA_CACHE_DIR"] = tempfile.mkdtemp(prefix="pre-grain-jit-", dir="/work")
assert not list(Path(os.environ["NUMBA_CACHE_DIR"]).iterdir())
sys.path.insert(0, "/opt/probe")
import numpy as np
from film import make_simulator, pixel_digest, render
import spektrafilm.model.grain as grain
from spektrafilm.utils.io import save_image_oiio

assert gc.isenabled()
thresholds = gc.get_threshold()
original_repeat = np.repeat
original_interpolate = grain.interp_density_cmy_layers
original_particles = grain.apply_grain_to_density_layers
original_collect = gc.collect
state = {"interpolating": False, "awaiting_particles": False, "refs": [], "times": []}


def repeat(*args, **kwargs):
    result = original_repeat(*args, **kwargs)
    if state["interpolating"]:
        assert result.dtype == np.float64 and result.flags.owndata
        state["refs"].append(weakref.ref(result))
    return result


def interpolate(*args, **kwargs):
    state["interpolating"] = True
    try:
        with patch.object(np, "repeat", repeat):
            layers = original_interpolate(*args, **kwargs)
    finally:
        state["interpolating"] = False
    state["layers_sha256"] = pixel_digest(layers)
    state["awaiting_particles"] = True
    return layers


def observed_collect(*args, **kwargs):
    # Delegates only an actual production invocation, adding no collection.
    started = time.perf_counter()
    count = original_collect(*args, **kwargs)
    if state["awaiting_particles"]:
        state["times"].append(time.perf_counter() - started)
    return count


def particles(layers, **kwargs):
    assert state["awaiting_particles"]
    assert len(state["refs"]) == 3, "Missing real repeated-argument observations"
    assert all(ref() is None for ref in state["refs"]), "Repeated input survives before grain"
    assert len(state["times"]) == 1, "Expected one production collection at the handoff"
    assert pixel_digest(layers) == state["layers_sha256"]
    assert gc.isenabled() and gc.get_threshold() == thresholds
    state["awaiting_particles"] = False
    return original_particles(layers, **kwargs)


# The unchanged two-render cold process from pipeline_reference_probe.py.
# These strict oracles predate this change, from the independently qualified #343
# reference image. The second render measures a warm in-process handoff; this
# does not reuse a compiled cache across attempts or create a new reference.
references = [
    (17, 19, "7ff1a228c01fa6e52024b3af79797444ac0d6ae5e3a3a49192616fcafbf60cce",
     "092b794c77bb504e67cf3b11cf28b91109857bcb81510946926eec86435fb792",
     "9030e30a29dc1e874e58a677a3302425f90bdb5ec815655abcd0ca6c00da7a03"),
    (128, 193, "894a8230d3e7eb76b9ee293a12d1b4cee63b4bf0f28254b4d0f98a6949ed5744",
     "192dd4eceb4e5d35bd3d0268b9a1bad1dc581fc26ee35590f353ede4f3917b02",
     "b87e8bc4d89a23366f02d7b8fb7510177648c3e106731a818ec604bfb220a692"),
]
simulator, _ = make_simulator()
with tempfile.TemporaryDirectory(prefix="pre-grain-jpeg-", dir="/work") as directory:
    for index, (height, width, source_sha, pixel_sha, jpeg_sha) in enumerate(references):
        state["refs"].clear()
        state["times"].clear()
        pixels = np.empty((height, width, 3), dtype=np.float32)
        pixels[:, :, 0] = np.linspace(.002, 1.4, width)[None, :]
        pixels[:, :, 1] = np.linspace(.05, 1, height)[:, None]
        pixels[:, :, 2] = .18
        assert pixel_digest(pixels) == source_sha
        started = time.perf_counter()
        with patch.object(grain, "interp_density_cmy_layers", interpolate), \
             patch.object(grain, "apply_grain_to_density_layers", particles), \
             patch.object(gc, "collect", observed_collect):
            result = render(simulator, pixels)
        elapsed = time.perf_counter() - started
        assert len(state["refs"]) == 3 and len(state["times"]) == 1
        assert not state["awaiting_particles"]
        assert pixel_digest(result) == pixel_sha
        path = Path(directory) / "result.jpg"
        save_image_oiio(str(path), result, color_space="sRGB", cctf_encoding=True)
        with path.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == jpeg_sha
        assert pixel_digest(pixels) == source_sha
        assert gc.isenabled() and gc.get_threshold() == thresholds
        print(json.dumps({"event": "pre_grain_reclamation", "iteration": index,
                          "shape": [height, width], "captured_backings": len(state["refs"]),
                          "reclaim_seconds": state["times"][0],
                          "inclusive_render_seconds": elapsed, "pixel_sha256": pixel_sha,
                          "jpeg_sha256": jpeg_sha, "gc_thresholds": thresholds}), flush=True)
        del result, pixels
