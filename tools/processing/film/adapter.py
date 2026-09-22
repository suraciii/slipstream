"""One fixed, independently referenced Film render inside a native attempt."""

import errno
import ctypes
import hashlib
import json
import os
import platform
import signal
from pathlib import Path
import stat
import sys

from contract import (
    ContractError, NUMERICAL_BUNDLE, OUTPUT_ICC, RECIPE,
    digest, read_grant, write_producer,
)

INPUT = Path("/input/input.tif")
OUTPUT = Path("/output/finished.jpg")
CACHE = Path("/work/numba")
MAX_DECODED_BYTES = 2 * 1024**3


def check_parent_isolation():
    """The child must not recover native PID 1's private result capability."""
    if os.getppid() != 1 or sys.platform != "linux":
        raise ContractError(None)
    syscall_numbers = {"x86_64": (434, 438), "aarch64": (434, 438)}.get(platform.machine())
    if syscall_numbers is None:
        raise ContractError(None)
    try:
        descriptor = os.open("/proc/1/fd", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EPERM):
            raise ContractError(None) from error
    else:
        os.close(descriptor)
        raise ContractError(None)
    libc = ctypes.CDLL(None, use_errno=True)
    ctypes.set_errno(0)
    pidfd = libc.syscall(ctypes.c_long(syscall_numbers[0]), ctypes.c_int(1), ctypes.c_uint(0))
    if pidfd < 0:
        if ctypes.get_errno() not in (errno.EACCES, errno.EPERM):
            raise ContractError(None)
    else:
        try:
            ctypes.set_errno(0)
            descriptor = libc.syscall(ctypes.c_long(syscall_numbers[1]), ctypes.c_int(pidfd),
                                      ctypes.c_int(0), ctypes.c_uint(0))
            if descriptor >= 0:
                os.close(descriptor)
                raise ContractError(None)
            if ctypes.get_errno() not in (errno.EACCES, errno.EPERM):
                raise ContractError(None)
        finally:
            os.close(pidfd)
    # SEIZE does not stop PID 1. No runtime switch disables this check.
    ctypes.set_errno(0)
    outcome = libc.ptrace(ctypes.c_uint(0x4206), ctypes.c_int(1),
                          ctypes.c_void_p(0), ctypes.c_void_p(0))
    if outcome != -1 or ctypes.get_errno() not in (errno.EACCES, errno.EPERM):
        raise ContractError(None)


def check_environment():
    expected = {
        "NUMBA_CACHE_DIR": str(CACHE), "NUMBA_NUM_THREADS": "1",
        "OMP_NUM_THREADS": "4", "OPENBLAS_NUM_THREADS": "4",
        "NUMEXPR_NUM_THREADS": "4",
    }
    if any(os.environ.get(key) != value for key, value in expected.items()):
        raise ContractError(None)
    # The native supervisor creates this fresh private directory before exec.
    # Check before importing any numerical module that can populate it.
    metadata = CACHE.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or any(CACHE.iterdir()):
        raise ContractError(None)
    if hashlib.sha256(Path("/opt/processing-bundle.json").read_bytes()).hexdigest() != NUMERICAL_BUNDLE:
        raise ContractError(None)
    if OUTPUT.exists() or OUTPUT.is_symlink():
        raise ContractError("artifact-invalid")


def check_plans(grant):
    """Recompute with the pinned numerical functions before frame allocation."""
    import numpy as np
    from spektrafilm.utils.bounded_gamut import plan_gamut_workspace
    from spektrafilm.utils.bounded_output import plan_cctf_workspace, plan_jpeg_workspace

    fixture, plans = grant["fixture"], grant["plan"]
    width, height = fixture["width"], fixture["height"]
    count = width * height
    if (plans["width"] != width or plans["height"] != height
            or not 1 <= width <= 9568 or not 1 <= height <= 9568
            or count * 12 > MAX_DECODED_BYTES or 3 * max(width, height) > 175000):
        raise ContractError()
    expected_cache = fixture["source"].get("bytes", 0)
    if plans["source_cache_bytes"] != expected_cache:
        raise ContractError()
    # CCTF shares the Simulator's gamut allowance. The fixed recipe includes it.
    if (plans["gamut"]["allowance_bytes"] != 603979776
            or plans["cctf"]["allowance_bytes"] != plans["gamut"]["allowance_bytes"]
            or plans["jpeg"]["allowance_bytes"] != 17825792):
        raise ContractError()
    # A read-only, zero-stride view carries geometry without allocating 24*N.
    shape_only = np.broadcast_to(np.zeros((1, 1, 3), dtype=np.float64), (height, width, 3))
    try:
        actual = {
            "gamut": plan_gamut_workspace(count, plans["gamut"]["allowance_bytes"]),
            "cctf": plan_cctf_workspace(count, plans["cctf"]["allowance_bytes"]),
            "jpeg": plan_jpeg_workspace(shape_only, plans["jpeg"]["allowance_bytes"]),
        }
    except ValueError as error:
        raise ContractError() from error
    for name, plan in actual.items():
        expected = {
            "model": plan.model, "allowance_bytes": plan.workspace_allowance_bytes,
            "scratch_bytes": plan.scratch_bytes, "batch_pixels": plan.batch_pixels,
            "destination_bytes": plan.destination_bytes,
        }
        if plans[name] != expected:
            raise ContractError()


def synthetic_pixels(fixture):
    import numpy as np

    source = fixture["source"]
    pattern, seed = source["pattern"], source["seed"]
    if pattern != "noise" and seed != 0:
        raise ContractError("unsupported-input")
    height, width = fixture["height"], fixture["width"]
    pixels = np.empty((height, width, 3), dtype=np.float32)
    if pattern in ("dark", "bright", "red", "green", "blue"):
        pixels.fill(4 if pattern == "bright" else 0)
        if pattern in ("red", "green", "blue"):
            pixels[:, :, ("red", "green", "blue").index(pattern)] = 4
    elif pattern == "gradient":
        count = height * width
        for first in range(0, count, 16384):
            last = min(first + 16384, count)
            indices = np.arange(first, last, dtype=np.float64)
            indices *= 4
            indices /= max(count - 1, 1)
            values = indices.astype(np.float32)
            pixels.reshape(-1, 3)[first:last] = values[:, None]
            del indices, values
    elif pattern == "noise":
        generator = np.random.Generator(np.random.PCG64(seed))
        flat = pixels.reshape(-1)
        for first in range(0, flat.size, 16384):
            block = flat[first:first + 16384]
            generator.random(dtype=np.float32, out=block)
            block *= np.float32(4)
    else:
        raise ContractError("unsupported-input")
    return pixels


def tiff_pixels(grant):
    import numpy as np
    import OpenImageIO as oiio

    fixture = grant["fixture"]
    metadata = INPUT.lstat()
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != 0
            or stat.S_IMODE(metadata.st_mode) != 0o444 or metadata.st_nlink != 1
            or metadata.st_size != fixture["source"]["bytes"]):
        raise ContractError("source-mismatch")
    reader = oiio.ImageInput.open(str(INPUT))
    if reader is None:
        raise ContractError("unsupported-input")
    try:
        spec = reader.spec()
        profile = spec.getattribute("ICCProfile")
        if (reader.format_name() != "tiff" or spec.deep
                or spec.nchannels != 3 or spec.format != oiio.FLOAT
                or spec.channelformats or spec.width != fixture["width"]
                or spec.height != fixture["height"] or spec.depth != 1
                or spec.x != 0 or spec.y != 0 or spec.z != 0
                or spec.full_width != spec.width or spec.full_height != spec.height
                or spec.full_depth != 1 or spec.get_int_attribute("Orientation", 1) != 1
                or profile is None or len(profile) > 16384
                or hashlib.sha256(bytes(profile)).hexdigest() != grant["input_icc_sha256"]):
            raise ContractError("unsupported-input")
        if reader.seek_subimage(1, 0):
            raise ContractError("unsupported-input")
        if not reader.seek_subimage(0, 0):
            raise ContractError("unsupported-input")
        pixels = reader.read_image(format=oiio.FLOAT)
        if (type(pixels) is not np.ndarray or pixels.dtype != np.dtype(np.float32)
                or pixels.shape != (fixture["height"], fixture["width"], 3)
                or not pixels.flags.c_contiguous):
            raise ContractError("unsupported-input")
    finally:
        closed = reader.close()
    if not closed:
        raise ContractError("unsupported-input")
    return pixels


def check_recipe(recipe):
    # This is the existing numerical recipe identity, which includes floats.
    encoded = json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()
    if hashlib.sha256(encoded).hexdigest() != RECIPE:
        raise ContractError()
    camera, io, settings = recipe["camera"], recipe["io"], recipe["settings"]
    grain = recipe["film_render"]["grain"]
    if (camera["film_format_mm"] != 35 or io["crop"] or io["upscale_factor"] != 1
            or settings["preview_mode"] or camera["diffusion_filter"]["active"]
            or recipe["enlarger"]["diffusion_filter"]["active"]
            or not settings["use_enlarger_lut"] or not settings["use_scanner_lut"]
            or settings["lut_resolution"] != 33 or not grain["active"]
            or not grain["sublayers_active"] or tuple(grain["micro_structure"]) != (0.2, 30)):
        raise ContractError()


def produce(grant):
    check_parent_isolation()
    check_environment()
    sys.path.insert(0, "/opt/probe")
    import OpenImageIO as oiio
    from film import make_simulator, pixel_digest, render
    from spektrafilm.utils.bounded_output import samples_are_finite
    from spektrafilm.utils.io import save_image_oiio

    oiio.attribute("threads", 4)
    check_plans(grant)
    fixture = grant["fixture"]
    pixels = (synthetic_pixels(fixture) if fixture["source"]["kind"] == "synthetic-rgb"
              else tiff_pixels(grant))
    if not samples_are_finite(pixels):
        raise ContractError("unsupported-input")
    input_hash = pixel_digest(pixels)
    if input_hash != fixture["reference"]["input_pixels_sha256"]:
        raise ContractError("source-mismatch")
    simulator, recipe = make_simulator(gamut_workspace_bytes=grant["plan"]["gamut"]["allowance_bytes"])
    check_recipe(recipe)
    result = render(simulator, pixels)
    film_hash = pixel_digest(result)
    if (film_hash != fixture["reference"]["film_pixels_sha256"]
            or pixel_digest(pixels) != input_hash
            or result.shape != pixels.shape):
        raise ContractError("artifact-invalid")
    # Python startup ignores SIGXFSZ. Restore the default so the native parent
    # can classify the kernel's exact file-limit signal even if OIIO drops errno.
    signal.signal(signal.SIGXFSZ, signal.SIG_DFL)
    try:
        save_image_oiio(str(OUTPUT), result, color_space="sRGB", cctf_encoding=True,
                        jpeg_workspace_bytes=grant["plan"]["jpeg"]["allowance_bytes"])
    except OSError:
        storage = os.statvfs(OUTPUT.parent)
        if storage.f_bavail == 0 or storage.f_favail == 0:
            raise OSError(errno.ENOSPC, "Attempt storage is exhausted") from None
        raise
    reader = oiio.ImageInput.open(str(OUTPUT))
    if reader is None:
        raise ContractError("artifact-invalid")
    try:
        spec = reader.spec()
        profile = spec.getattribute("ICCProfile")
        if (reader.format_name() != "jpeg" or spec.width != fixture["width"]
                or spec.height != fixture["height"] or spec.nchannels != 3
                or profile is None or len(profile) > 16384
                or hashlib.sha256(bytes(profile)).hexdigest() != OUTPUT_ICC):
            raise ContractError("artifact-invalid")
    finally:
        closed = reader.close()
    if not closed:
        raise ContractError("artifact-invalid")
    return {
        "outcome": "produced", "stages": [],
        "pixels": {"input_pixels_sha256": input_hash, "film_pixels_sha256": film_hash,
                   "width": fixture["width"], "height": fixture["height"], "icc_sha256": OUTPUT_ICC},
    }


def main():
    if sys.argv[1:]:
        return 75
    # No trusted identities exist if this fails; emit no purported result.
    grant = read_grant()
    result = {"version": 2, "kind": "film-producer-result", "launch_id": grant["launch_id"],
              "manifest": grant["manifest"], "plan_sha256": digest(grant["plan"])}
    try:
        result.update(produce(grant))
    except MemoryError:
        result.update(outcome="allocation-failed", detail=None)
    except ContractError as error:
        result.update(outcome="engine-failed", detail=error.detail)
    except OSError as error:
        if error.errno in (errno.ENOSPC, errno.EDQUOT, errno.EFBIG):
            result.update(outcome="storage-full", detail="output-limit" if error.errno == errno.EFBIG else None)
        else:
            result.update(outcome="engine-failed", detail=None)
    except Exception:
        result.update(outcome="engine-failed", detail=None)
    write_producer(result)
    return {"produced": 0, "allocation-failed": 20, "storage-full": 21,
            "engine-failed": 75}[result["outcome"]]


if __name__ == "__main__":
    raise SystemExit(main())
