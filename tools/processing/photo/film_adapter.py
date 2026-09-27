"""Pinned film-jpeg adapter (production worker side).

Stage two of the `film-jpeg` workload. The Development TIFF produced by
`adapter.py` inside the private attempt workspace is validated against the
pinned handoff contract — IEEE float32 RGB samples, bounded full geometry,
and the exact embedded linear ProPhoto ICC bytes of the shared Film identity —
then rendered once through the fixed Film recipe and published as a
quality-85 sRGB JPEG.

The fixed recipe, seed behavior and Finished JPEG encoding are reused from
the qualified development runtime (`film.py`, `finished_jpeg.py`); every
pinned identity comes from the one authoritative `film_identity.py`. This
adapter adds no recipe of its own: any recipe or identity mismatch refuses
the attempt before a single pixel is rendered.
"""

import argparse
import errno
import hashlib
import json
import os
import stat
import sys
from pathlib import Path

# In the pinned image the shared development runtime lives at /opt/probe; on a
# development host it is the sibling tools/development tree of this repository.
_PROBE = Path("/opt/probe")
if (_PROBE / "film_identity.py").is_file():
    sys.path.insert(0, str(_PROBE))
else:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "development"))

from film_identity import (  # noqa: E402
    FILM_RECIPE_SHA256 as RECIPE_SHA256,
    FINISHED_JPEG_QUALITY,
    INPUT_ICC_SHA256,
    OUTPUT_ICC_SHA256,
)

PRODUCED = 0
ENGINE_FAILED = 1
# The pinned-identity refusal: the attempt is rejected before any engine work,
# exactly like the development adapter's handoff-profile refusal.
REFUSED = 71

# The qualified geometry bounds, mirroring the plan checks of the
# independently qualified Film adapter, so no attempt can hand the pinned
# numerical runtime an unbounded frame.
MAX_DECODED_BYTES = 2 * 1024**3
MAX_EDGE = 9568
MAX_COORDINATE_SUM = 175_000
NUMBA_CACHE = Path("/work/numba")


class Refusal(Exception):
    """A pinned-identity violation: no engine work may be trusted after it."""


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def geometry_bounds(width, height):
    """The bounded frame the pinned workspace plans are computed for."""
    return (
        1 <= width <= MAX_EDGE
        and 1 <= height <= MAX_EDGE
        and width * height * 12 <= MAX_DECODED_BYTES
        and 3 * max(width, height) <= MAX_COORDINATE_SUM
    )


def profile_is_pinned(profile, expected_sha256):
    """Exact embedded profile bytes; a lookalike profile is not evidence."""
    return (
        profile is not None
        and 0 < len(profile) <= 16384
        and sha256_bytes(bytes(profile)) == expected_sha256
    )


def input_spec_is_pinned(spec, sample_format, profile):
    """One full-frame float32 RGB image with the pinned handoff profile."""
    return (
        spec.nchannels == 3
        and not spec.channelformats
        and sample_format == "float"
        and spec.depth == 1
        and spec.x == 0
        and spec.y == 0
        and spec.z == 0
        and spec.full_width == spec.width
        and spec.full_height == spec.height
        and spec.full_depth == 1
        and spec.get_int_attribute("Orientation", 1) == 1
        and geometry_bounds(spec.width, spec.height)
        and profile_is_pinned(profile, INPUT_ICC_SHA256)
    )


def finished_spec_is_pinned(format_name, spec, profile, width, height):
    """The finished JPEG keeps the input geometry and pins the output sRGB."""
    return (
        format_name == "jpeg"
        and spec.nchannels == 3
        and spec.width == width
        and spec.height == height
        and profile_is_pinned(profile, OUTPUT_ICC_SHA256)
    )


def check_parent_isolation():
    """The adapter must not recover its PID 1's private result capability.

    The pinned worker clears PR_SET_DUMPABLE before releasing the engine, so
    the attempt boundary denies its own child every inspection interface; a
    child that can inspect PID 1 is not running inside that boundary.
    """
    if os.getppid() != 1 or sys.platform != "linux":
        return False
    try:
        descriptor = os.open("/proc/1/fd", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EPERM):
            return False
    else:
        os.close(descriptor)
        return False
    import ctypes

    libc = ctypes.CDLL(None, use_errno=True)
    ctypes.set_errno(0)
    pidfd = libc.syscall(ctypes.c_long(434), ctypes.c_int(1), ctypes.c_uint(0))
    if pidfd < 0:
        return ctypes.get_errno() in (errno.EACCES, errno.EPERM)
    try:
        ctypes.set_errno(0)
        borrowed = libc.syscall(
            ctypes.c_long(438), ctypes.c_int(pidfd), ctypes.c_int(0), ctypes.c_uint(0)
        )
        if borrowed >= 0:
            os.close(borrowed)
            return False
        return ctypes.get_errno() in (errno.EACCES, errno.EPERM)
    finally:
        os.close(pidfd)


def check_environment():
    """The deterministic one-thread Numba runtime with a fresh private cache."""
    expected = {
        "NUMBA_CACHE_DIR": str(NUMBA_CACHE),
        "NUMBA_NUM_THREADS": "1",
        "OMP_NUM_THREADS": "4",
        "OPENBLAS_NUM_THREADS": "4",
        "NUMEXPR_NUM_THREADS": "4",
    }
    if any(os.environ.get(key) != value for key, value in expected.items()):
        return False
    try:
        metadata = NUMBA_CACHE.lstat()
        empty = not any(NUMBA_CACHE.iterdir())
    except OSError:
        return False
    return stat.S_ISDIR(metadata.st_mode) and empty


def development_pixels(input_path):
    """Read the launcher-owned Development TIFF and verify the handoff."""
    import OpenImageIO as oiio

    metadata = input_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise Refusal("development source identity is invalid")
    reader = oiio.ImageInput.open(str(input_path))
    if reader is None:
        raise Refusal("development source is not readable")
    try:
        spec = reader.spec()
        if not input_spec_is_pinned(spec, str(spec.format), spec.getattribute("ICCProfile")):
            raise Refusal("development source misses the pinned handoff identity")
        if reader.seek_subimage(1, 0):
            raise Refusal("development source must be a single-frame TIFF")
        if not reader.seek_subimage(0, 0):
            raise Refusal("development source is not readable")
        pixels = reader.read_image(format=oiio.FLOAT)
    finally:
        closed = reader.close()
    if not closed:
        raise Refusal("development source is not readable")
    import numpy as np

    if (
        type(pixels) is not np.ndarray
        or pixels.dtype != np.dtype(np.float32)
        or pixels.shape != (spec.height, spec.width, 3)
        or not pixels.flags.c_contiguous
    ):
        raise Refusal("development source is not a float32 RGB frame")
    return pixels, (spec.width, spec.height)


def check_recipe(recipe):
    """Re-verify the fixed recipe of the shared engine against the identity."""
    import film

    encoded = json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()
    if sha256_bytes(encoded) != RECIPE_SHA256:
        raise Refusal("film recipe identity does not match the pinned bundle")
    camera, io, settings, debug = (
        recipe["camera"],
        recipe["io"],
        recipe["settings"],
        recipe["debug"],
    )
    grain = recipe["film_render"]["grain"]
    if (
        camera["auto_exposure"]
        or camera["exposure_compensation_ev"] != 0.0
        or io["input_color_space"] != "ProPhoto RGB"
        or io["input_cctf_decoding"]
        or io["output_color_space"] != "sRGB"
        or not io["output_cctf_encoding"]
        or settings["preview_mode"]
        or settings["use_fast_stats"]
        or not settings["use_enlarger_lut"]
        or not settings["use_scanner_lut"]
        or settings["lut_resolution"] != 33
        or debug["deactivate_spatial_effects"]
        or debug["deactivate_stochastic_effects"]
        or not grain["active"]
        or not grain["sublayers_active"]
        or recipe["seed"] != film.SEED
        or FINISHED_JPEG_QUALITY != 85
    ):
        raise Refusal("film recipe does not pin the qualified procedure")


def check_plans(width, height):
    """Recompute the bounded workspace plans before any frame allocation."""
    import numpy as np
    from spektrafilm.utils.bounded_gamut import (
        MAX_WORKSPACE_BYTES,
        plan_gamut_workspace,
    )
    from spektrafilm.utils.bounded_output import (
        JPEG_WORKSPACE_BYTES,
        plan_cctf_workspace,
        plan_jpeg_workspace,
    )

    # A read-only, zero-stride view carries geometry without allocating 24*N.
    shape_only = np.broadcast_to(
        np.zeros((1, 1, 3), dtype=np.float64), (height, width, 3)
    )
    for plan in (
        plan_gamut_workspace(width * height, MAX_WORKSPACE_BYTES),
        plan_cctf_workspace(width * height, MAX_WORKSPACE_BYTES),
        plan_jpeg_workspace(shape_only, JPEG_WORKSPACE_BYTES),
    ):
        if plan.scratch_bytes > plan.workspace_allowance_bytes or plan.batch_pixels <= 0:
            raise Refusal("the pinned workspace does not admit this geometry")


def produce(input_path, output_path):
    if not check_parent_isolation():
        raise Refusal("attempt boundary does not isolate the engine")
    if not check_environment():
        raise Refusal("engine environment is not the pinned deterministic one")

    pixels, (width, height) = development_pixels(input_path)
    from spektrafilm.utils.bounded_output import samples_are_finite

    if not samples_are_finite(pixels):
        raise RuntimeError("development source carries non-finite samples")
    check_plans(width, height)

    from film import make_simulator, render

    simulator, recipe = make_simulator()
    check_recipe(recipe)
    result = render(simulator, pixels)
    if result.shape != pixels.shape or not samples_are_finite(result):
        raise RuntimeError("film processing returned an invalid frame")
    # Python startup ignores SIGXFSZ; restore the kernel's default so a file
    # limit is reported instead of silently truncating the finished artifact.
    import signal

    signal.signal(signal.SIGXFSZ, signal.SIG_DFL)
    from finished_jpeg import save_finished_jpeg

    output_path.parent.mkdir(parents=True, exist_ok=True)
    from spektrafilm.utils.bounded_output import JPEG_WORKSPACE_BYTES

    save_finished_jpeg(str(output_path), result, workspace_bytes=JPEG_WORKSPACE_BYTES)
    verify_finished(output_path, width, height)


def verify_finished(output_path, width, height):
    """The published artifact must decode as the pinned sRGB finished JPEG."""
    import OpenImageIO as oiio

    metadata = output_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        raise RuntimeError("finished JPEG was not written")
    reader = oiio.ImageInput.open(str(output_path))
    if reader is None:
        raise RuntimeError("finished JPEG is not readable")
    try:
        spec = reader.spec()
        if not finished_spec_is_pinned(
            reader.format_name(),
            spec,
            spec.getattribute("ICCProfile"),
            width,
            height,
        ):
            raise RuntimeError("finished JPEG misses the pinned output identity")
    finally:
        closed = reader.close()
    if not closed:
        raise RuntimeError("finished JPEG is not readable")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    # Refuse before any engine or numerical import: without the pinned handoff
    # source or the deterministic environment there is no trusted identity,
    # so no purported result is written.
    if not args.input.is_file() or args.output.exists() or args.output.is_symlink():
        print(f"refusing to render without the sealed development source: {args.input}",
              file=sys.stderr)
        return REFUSED
    if not check_environment():
        print("refusing to render outside the pinned deterministic engine environment",
              file=sys.stderr)
        return REFUSED
    try:
        produce(args.input, args.output)
    except Refusal as error:
        print(f"refusing the pinned film contract: {error}", file=sys.stderr)
        return REFUSED
    except (RuntimeError, OSError, MemoryError) as error:
        print(f"film stage failed: {error}", file=sys.stderr)
        return ENGINE_FAILED
    return PRODUCED


if __name__ == "__main__":
    raise SystemExit(main())
