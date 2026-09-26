"""Compare the fixed internal LUT path with direct spectral evaluation."""

from copy import deepcopy
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import sys
import time

sys.path.insert(0, "/opt/probe")

_CACHE_DIR = Path(os.environ.get("NUMBA_CACHE_DIR", ""))
assert _CACHE_DIR.is_dir() and not any(_CACHE_DIR.iterdir()), (
    "LUT quality comparison requires an empty private Numba cache"
)

import colour
import numpy as np

from film_identity import FILM_RECIPE_SHA256
from spektrafilm import Simulator, digest_params, init_params
from spektrafilm.utils.bounded_gamut import MAX_WORKSPACE_BYTES

from bundle import load_bundle
from film import make_simulator, pixel_digest, render
from lut_quality import (
    LUT_CIEDE2000_MAX,
    LUT_CIEDE2000_P95_MAX,
    LUT_QUALITY_CRITERION,
    comparison_corpus,
    ciede2000_passes,
    nearest_rank,
    summarize_difference,
)



def make_direct_simulator(*, gamut_workspace_bytes=MAX_WORKSPACE_BYTES):
    """Build the accepted recipe with only the two engine LUTs disabled."""
    params = init_params("kodak_portra_400", "kodak_portra_endura")
    params.camera.auto_exposure = False
    params.camera.exposure_compensation_ev = 0.0
    params.io.input_color_space = "ProPhoto RGB"
    params.io.input_cctf_decoding = False
    params.io.output_color_space = "sRGB"
    params.io.output_cctf_encoding = True
    params.settings.preview_mode = False
    params.settings.use_fast_stats = False
    params.settings.use_enlarger_lut = False
    params.settings.use_scanner_lut = False
    params.settings.lut_resolution = 33
    params.debug.deactivate_spatial_effects = False
    params.debug.deactivate_stochastic_effects = False
    params.debug.lut_mode = False
    params = digest_params(params)
    manifest = {
        key: asdict(getattr(params, key))
        for key in (
            "camera", "enlarger", "scanner", "io", "settings", "debug",
            "film_render", "print_render", "taps",
        )
    }
    manifest.update(film="kodak_portra_400", paper="kodak_portra_endura", seed=327)
    manifest["processing_bundle"] = load_bundle()
    manifest["gamut_workspace_allowance_bytes"] = gamut_workspace_bytes
    return Simulator(params, output_gamut_workspace_bytes=gamut_workspace_bytes), manifest


def recipe_digest(recipe):
    encoded = json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def ciede2000_summary(reference, candidate):
    """Compare encoded sRGB outputs using a fixed D65 CIEDE2000 conversion."""
    colourspace = colour.RGB_COLOURSPACES["sRGB"]
    reference_xyz = colour.RGB_to_XYZ(
        np.clip(reference, 0.0, 1.0), colourspace,
        chromatic_adaptation_transform=None, apply_cctf_decoding=True,
    )
    candidate_xyz = colour.RGB_to_XYZ(
        np.clip(candidate, 0.0, 1.0), colourspace,
        chromatic_adaptation_transform=None, apply_cctf_decoding=True,
    )
    reference_lab = colour.XYZ_to_Lab(reference_xyz, illuminant=colourspace.whitepoint)
    candidate_lab = colour.XYZ_to_Lab(candidate_xyz, illuminant=colourspace.whitepoint)
    difference = np.asarray(colour.delta_E(reference_lab, candidate_lab, method="CIE 2000"))
    return {
        "mean": float(np.mean(difference)),
        "p95": nearest_rank(difference, 0.95),
        "max": float(np.max(difference)),
    }


def main():
    started = time.monotonic()
    lut_simulator, lut_recipe = make_simulator()
    direct_simulator, direct_recipe = make_direct_simulator()
    expected_direct_recipe = deepcopy(lut_recipe)
    expected_direct_recipe["settings"]["use_enlarger_lut"] = False
    expected_direct_recipe["settings"]["use_scanner_lut"] = False
    assert direct_recipe == expected_direct_recipe, "direct comparison changed more than LUT settings"

    lut_manifest_sha = recipe_digest(lut_recipe)
    recipe_identity_matches = lut_manifest_sha == FILM_RECIPE_SHA256
    blocking_reasons = []
    if not recipe_identity_matches:
        blocking_reasons.append("qualification recipe differs from adapter recipe identity")

    rows = []
    for name, pixels in comparison_corpus():
        direct_first = render(direct_simulator, pixels)
        direct_second = render(direct_simulator, pixels)
        lut_first = render(lut_simulator, pixels)
        lut_second = render(lut_simulator, pixels)
        assert pixel_digest(direct_first) == pixel_digest(direct_second), (
            f"direct spectral output changed on repeat: {name}"
        )
        assert pixel_digest(lut_first) == pixel_digest(lut_second), (
            f"LUT output changed on repeat: {name}"
        )
        metrics = summarize_difference(direct_first, lut_first)
        metrics["ciede2000"] = ciede2000_summary(direct_first, lut_first)
        rows.append({
            "case": name,
            "geometry": list(pixels.shape[:2]),
            "direct_spectral_pixel_sha256": pixel_digest(direct_first),
            "lut_pixel_sha256": pixel_digest(lut_first),
            "metrics": metrics,
        })

    criterion_passes = all(
        ciede2000_passes(row["metrics"]["ciede2000"])
        for row in rows
    )
    if not criterion_passes:
        blocking_reasons.append("lut-ciede2000-threshold-exceeded")

    print(json.dumps({
        "comparison": "lut-vs-direct-spectral-v1",
        "criterion": LUT_QUALITY_CRITERION,
        "criterion_limits": {
            "ciede2000_p95_max": LUT_CIEDE2000_P95_MAX,
            "ciede2000_max": LUT_CIEDE2000_MAX,
        },
        "criterion_passes": criterion_passes,
        "criterion_scope": "synthetic-smoke",
        "adapter_recipe_sha256": FILM_RECIPE_SHA256,
        "lut_manifest_sha256": lut_manifest_sha,
        "direct_manifest_sha256": recipe_digest(direct_recipe),
        "recipe_identity_matches": recipe_identity_matches,
        "blocking_reasons": blocking_reasons,
        "cache_state": "empty-at-process-start",
        "cases": rows,
        "seconds": time.monotonic() - started,
        "acceptance": False,
        "note": "The selected criterion passes only on this synthetic smoke corpus; representative camera-derived and full-resolution evidence remain required for Film acceptance.",
    }, sort_keys=True))


if __name__ == "__main__":
    main()
