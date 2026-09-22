"""Qualification-only fixed recipe. This is not the service processing adapter."""

import hashlib
from dataclasses import asdict

import numba
import numpy as np
from spektrafilm import Simulator, digest_params, init_params

SEED = 327


@numba.njit(cache=True)
def seed_numba(seed):
    # Interpreted NumPy seeding does not initialize Numba's independent RNG.
    np.random.seed(seed)


def reset_random_state():
    if numba.get_num_threads() != 1:
        raise RuntimeError("The qualification recipe requires one Numba thread")
    np.random.seed(SEED)
    seed_numba(SEED)


def make_simulator():
    params = init_params("kodak_portra_400", "kodak_portra_endura")
    params.camera.auto_exposure = False
    params.camera.exposure_compensation_ev = 0.0
    params.io.input_color_space = "ProPhoto RGB"
    params.io.input_cctf_decoding = False
    params.io.output_color_space = "sRGB"
    params.io.output_cctf_encoding = True
    params.settings.preview_mode = False
    params.settings.use_fast_stats = False
    params.settings.use_enlarger_lut = True
    params.settings.use_scanner_lut = True
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
    manifest.update(film="kodak_portra_400", paper="kodak_portra_endura", seed=SEED)
    return Simulator(params), manifest


def render(simulator, pixels):
    reset_random_state()
    result = simulator.process(pixels)
    if not np.isfinite(result).all():
        raise RuntimeError("Film processing returned non-finite samples")
    return result


def pixel_digest(pixels):
    return hashlib.sha256(pixels.tobytes()).hexdigest()
