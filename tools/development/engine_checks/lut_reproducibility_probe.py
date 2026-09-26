"""Check exact LUT/direct repeatability across fresh processes and A/B/A."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


sys.path.insert(0, "/opt/probe")

_CACHE_DIR = Path(os.environ.get("NUMBA_CACHE_DIR", ""))
assert _CACHE_DIR.is_dir() and not any(_CACHE_DIR.iterdir()), (
    "Fresh-process repeatability requires an empty private parent cache"
)

from film_identity import FILM_RECIPE_SHA256


SEQUENCE = ("lut", "direct", "lut", "direct")


def _manifest_digest(recipe):
    return hashlib.sha256(
        json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _effects(recipe):
    return {
        "grain_active": recipe["film_render"]["grain"]["active"],
        "halation_active": recipe["film_render"]["halation"]["active"],
        "glare_active": recipe["print_render"]["glare"]["active"],
        "preview_mode": recipe["settings"]["preview_mode"],
        "spatial_effects_deactivated": recipe["debug"]["deactivate_spatial_effects"],
        "stochastic_effects_deactivated": recipe["debug"]["deactivate_stochastic_effects"],
    }


def _simulators(mode):
    from lut_quality_probe import make_direct_simulator
    from film import make_simulator

    if mode == "lut":
        return make_simulator(), None
    if mode == "direct":
        return make_direct_simulator(), None
    if mode == "aba":
        lut_simulator, lut_recipe = make_simulator()
        direct_simulator, _ = make_direct_simulator()
        return (lut_simulator, lut_recipe), direct_simulator
    raise ValueError(f"unknown comparison mode: {mode}")


def _child(mode):
    (simulator, recipe), alternate = _simulators(mode)
    from film import pixel_digest, render
    from lut_quality import comparison_corpus
    if mode == "aba":
        rows = []
        for name, pixels in comparison_corpus():
            before = pixel_digest(render(simulator, pixels))
            render(alternate, pixels)
            after = pixel_digest(render(simulator, pixels))
            if before != after:
                raise AssertionError(f"LUT output changed in one A/B/A process: {name}")
            rows.append({
                "case": name,
                "geometry": list(pixels.shape[:2]),
                "pixel_sha256": before,
            })
        print(json.dumps({
            "mode": mode,
            "manifest_sha256": _manifest_digest(recipe),
            "in_process_repeatable": True,
            "in_process_a_b_a": True,
            "effects": _effects(recipe),
            "cases": rows,
        }, sort_keys=True))
        return

    rows = []
    for name, pixels in comparison_corpus():
        first = render(simulator, pixels)
        second = render(simulator, pixels)
        first_digest = pixel_digest(first)
        second_digest = pixel_digest(second)
        if first_digest != second_digest:
            raise AssertionError(f"{mode} output changed in one process: {name}")
        rows.append({
            "case": name,
            "geometry": list(pixels.shape[:2]),
            "pixel_sha256": first_digest,
        })

    print(json.dumps({
        "mode": mode,
        "manifest_sha256": _manifest_digest(recipe),
        "in_process_repeatable": True,
        "in_process_a_b_a": False,
        "effects": _effects(recipe) if mode == "lut" else None,
        "cases": rows,
    }, sort_keys=True))


def _run_child(mode):
    with tempfile.TemporaryDirectory(dir="/work", prefix=f"lut-repro-{mode}-") as cache:
        result = subprocess.run(
            [sys.executable, __file__, "--child", mode],
            check=True,
            capture_output=True,
            text=True,
            env={**os.environ, "NUMBA_CACHE_DIR": cache},
        )
    return json.loads(result.stdout)


def _same_cases(left, right):
    return [row["pixel_sha256"] for row in left["cases"]] == [
        row["pixel_sha256"] for row in right["cases"]
    ]


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        _child(sys.argv[2])
        return
    if len(sys.argv) != 1:
        raise SystemExit("usage: lut_reproducibility_probe.py [--child lut|direct|aba]")

    runs = [_run_child(mode) for mode in SEQUENCE]
    in_process_aba = _run_child("aba")
    lut_runs = [runs[index] for index, mode in enumerate(SEQUENCE) if mode == "lut"]
    direct_runs = [runs[index] for index, mode in enumerate(SEQUENCE) if mode == "direct"]
    lut_manifest = lut_runs[0]["manifest_sha256"]
    direct_manifest = direct_runs[0]["manifest_sha256"]
    recipe_identity_matches = lut_manifest == FILM_RECIPE_SHA256
    blocking_reasons = [] if recipe_identity_matches else [
        "qualification recipe differs from adapter recipe identity",
    ]
    if not all(run["manifest_sha256"] == lut_manifest for run in lut_runs):
        raise AssertionError("LUT recipe identity changed across fresh processes")
    if in_process_aba["manifest_sha256"] != lut_manifest:
        raise AssertionError("in-process A/B/A used a different LUT recipe")
    if not all(run["manifest_sha256"] == direct_manifest for run in direct_runs):
        raise AssertionError("direct recipe identity changed across fresh processes")
    if not all(run["in_process_repeatable"] for run in runs):
        raise AssertionError("a child process failed in-process repeatability")
    if not in_process_aba["in_process_a_b_a"]:
        raise AssertionError("in-process A/B/A did not complete")
    if not _same_cases(direct_runs[0], direct_runs[1]):
        raise AssertionError("direct output changed across fresh processes")
    if not _same_cases(lut_runs[0], lut_runs[1]):
        raise AssertionError("LUT A/B/A result changed across fresh processes")
    if not _same_cases(lut_runs[0], in_process_aba):
        raise AssertionError("in-process A/B/A differs from the fresh LUT result")

    effects = lut_runs[0]["effects"]
    if effects != {
        "grain_active": True,
        "halation_active": True,
        "glare_active": True,
        "preview_mode": False,
        "spatial_effects_deactivated": False,
        "stochastic_effects_deactivated": False,
    }:
        raise AssertionError(f"full-effect recipe was not exercised: {effects}")

    from lut_quality import (
        LUT_CIEDE2000_MAX,
        LUT_CIEDE2000_P95_MAX,
        LUT_QUALITY_CRITERION,
    )

    print(json.dumps({
        "comparison": "lut-vs-direct-spectral-reproducibility-v1",
        "procedure": "film-once-empty-cache-v1",
        "sequence": list(SEQUENCE),
        "in_process_sequence": ["lut", "direct", "lut"],
        "recipe_identity_matches": recipe_identity_matches,
        "blocking_reasons": blocking_reasons,
        "fresh_process_repeatability": {
            "lut": True,
            "direct": True,
        },
        "in_process_repeatability": True,
        "a_b_a": {
            "lut_before_after_direct_match": True,
            "direct_runs_match": True,
            "in_process_match": True,
        },
        "effects": effects,
        "lut_manifest_sha256": lut_manifest,
        "direct_manifest_sha256": direct_manifest,
        "cases": [
            {
                "case": row["case"],
                "geometry": row["geometry"],
                "lut_pixel_sha256": lut_runs[0]["cases"][index]["pixel_sha256"],
                "direct_pixel_sha256": direct_runs[0]["cases"][index]["pixel_sha256"],
            }
            for index, row in enumerate(lut_runs[0]["cases"])
        ],
        "acceptance": False,
        "criterion": LUT_QUALITY_CRITERION,
        "criterion_limits": {
            "ciede2000_p95_max": LUT_CIEDE2000_P95_MAX,
            "ciede2000_max": LUT_CIEDE2000_MAX,
        },
        "criterion_passes": None,
        "criterion_scope": "not-measured-by-repeatability",
        "note": "Repeatability evidence only; the selected criterion still requires representative camera-derived and full-resolution quality measurements.",
    }, sort_keys=True))


if __name__ == "__main__":
    main()
