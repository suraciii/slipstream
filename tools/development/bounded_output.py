"""Bound pointwise display encoding, JPEG conversion, and sample observation.

Workspace describes numerical buffers only. The complete stage model must also
reserve caller input, the returned destination, and native codec/runtime memory.
"""

from dataclasses import dataclass
import sys

import colour
import numpy as np

MAX_BATCH_PIXELS = 262144
OBSERVATION_CHUNK_SAMPLES = 3 * MAX_BATCH_PIXELS
CCTF_FIXED_BYTES = 4 * 1024 * 1024
CCTF_BYTES_PER_PIXEL = 256
JPEG_FIXED_BYTES = 1024 * 1024
JPEG_BYTES_PER_PIXEL = 64
JPEG_WORKSPACE_BYTES = JPEG_FIXED_BYTES + JPEG_BYTES_PER_PIXEL * MAX_BATCH_PIXELS


@dataclass(frozen=True)
class OutputWorkspacePlan:
    model: str
    pixel_count: int
    destination_bytes: int
    workspace_allowance_bytes: int
    scratch_bytes: int
    batch_pixels: int


def _positive_integer(value, label):
    if type(value) is not int or value <= 0 or value > sys.maxsize:
        raise ValueError(f"{label} must be a positive platform-sized integer")


def _plan(pixel_count, workspace_bytes, *, model, fixed, per_pixel, destination_pixel_bytes):
    _positive_integer(pixel_count, "pixel_count")
    _positive_integer(workspace_bytes, "workspace_bytes")
    if pixel_count > sys.maxsize // max(1, destination_pixel_bytes):
        raise ValueError("Output destination byte count exceeds the allocation range")
    if workspace_bytes < fixed + per_pixel:
        raise ValueError(f"{model} workspace requires at least {fixed + per_pixel} bytes")
    batch = min(pixel_count, MAX_BATCH_PIXELS, (workspace_bytes - fixed) // per_pixel)
    return OutputWorkspacePlan(model, pixel_count, pixel_count * destination_pixel_bytes,
                               workspace_bytes, fixed + batch * per_pixel, batch)


def plan_cctf_workspace(pixel_count, workspace_bytes):
    return _plan(pixel_count, workspace_bytes, model="srgb-cctf-f64-v1",
                 fixed=CCTF_FIXED_BYTES, per_pixel=CCTF_BYTES_PER_PIXEL,
                 destination_pixel_bytes=24)


def encode_rgb_bounded(rgb, *, output_color_space, workspace_bytes):
    if output_color_space != "sRGB":
        raise ValueError("Bounded display encoding requires sRGB")
    if (type(rgb) is not np.ndarray or rgb.dtype != np.dtype(np.float64)
            or rgb.ndim < 1 or rgb.shape[-1] != 3 or not rgb.flags.c_contiguous):
        raise ValueError("Display encoding requires C-contiguous native float64 RGB")
    plan = plan_cctf_workspace(rgb.size // 3, workspace_bytes)
    source = rgb.reshape(-1, 3)
    result = np.empty(rgb.shape, dtype=np.float64)
    destination = result.reshape(-1, 3)
    for first in range(0, plan.pixel_count, plan.batch_pixels):
        last = min(first + plan.batch_pixels, plan.pixel_count)
        # Same-profile RGB_to_RGB includes its matrix operation. A standalone
        # sRGB transfer-function call is not the exact qualified transform.
        destination[first:last] = colour.RGB_to_RGB(
            source[first:last], output_color_space, output_color_space,
            apply_cctf_decoding=False, apply_cctf_encoding=True,
        )
    return result


def plan_jpeg_workspace(image, workspace_bytes=JPEG_WORKSPACE_BYTES):
    if (type(image) is not np.ndarray or image.ndim != 3 or image.shape[2] != 3
            or image.dtype not in (np.dtype(np.float32), np.dtype(np.float64))):
        raise ValueError("JPEG conversion requires native float32 or float64 RGB")
    height, width, _ = image.shape
    _positive_integer(height, "height")
    _positive_integer(width, "width")
    if width > sys.maxsize // height:
        raise ValueError("JPEG pixel count exceeds the allocation range")
    plan = _plan(height * width, workspace_bytes, model="jpeg-uint8-rows-v1",
                 fixed=JPEG_FIXED_BYTES, per_pixel=JPEG_BYTES_PER_PIXEL,
                 destination_pixel_bytes=0)
    rows = plan.batch_pixels // width
    if rows < 1:
        raise ValueError("JPEG workspace cannot admit one full-width row")
    batch_pixels = rows * width
    return OutputWorkspacePlan(plan.model, plan.pixel_count, 0, workspace_bytes,
                               JPEG_FIXED_BYTES + batch_pixels * JPEG_BYTES_PER_PIXEL,
                               batch_pixels)


def jpeg_row_batches(image, plan):
    """Strides need only bounded normalization of each returned uint8 batch."""
    height, width, _ = image.shape
    rows = plan.batch_pixels // width
    for first in range(0, height, rows):
        last = min(first + rows, height)
        # Keep the original operation order and dtype-dependent arithmetic.
        pixels = (np.clip(image[first:last], 0, 1) * 255.0).astype(np.uint8)
        pixels = np.ascontiguousarray(pixels)
        yield first, last, pixels


def c_order_chunks(pixels, *, chunk_samples=OBSERVATION_CHUNK_SAMPLES):
    """Yield bounded contiguous views/buffers with exact ndarray.tobytes order."""
    if (type(pixels) is not np.ndarray or pixels.dtype.kind not in "buifc"
            or pixels.dtype.itemsize > 16):
        raise ValueError("Sample observation requires a numeric ndarray")
    _positive_integer(chunk_samples, "chunk_samples")
    if chunk_samples > OBSERVATION_CHUNK_SAMPLES:
        raise ValueError("Sample observation chunk exceeds its qualified range")
    with np.nditer(pixels, order="C", flags=["external_loop", "buffered", "zerosize_ok"],
                   op_flags=["readonly", "contig"], buffersize=chunk_samples) as iterator:
        for chunk in iterator:
            yield chunk


def samples_are_finite(pixels):
    return all(bool(np.isfinite(chunk).all()) for chunk in c_order_chunks(pixels))
