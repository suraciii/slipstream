"""Bound the pinned CAM16-UCS transform without changing its arithmetic.

This transform-local model does not admit an entire Film attempt. The caller
must reserve its input, destination, other live buffers and runtime headroom
before granting workspace_bytes. The kernel remains the hard-limit boundary.
"""

from dataclasses import dataclass
import sys

import numpy as np

from spektrafilm.utils.gamut_compression import OutputGamutCompressSpec, compress_rgb

MODEL_VERSION = "cam16ucs-srgb-f64-v1"
# Conservative measured allowances for the pinned numerical bundle. Fixed bytes
# cover cold 64x720 color-table construction; per-pixel bytes cover overlapping
# colour-science temporaries and the returned batch. See engine_checks.
FIXED_SCRATCH_BYTES = 64 * 1024 * 1024
SCRATCH_BYTES_PER_PIXEL = 2048
MAX_BATCH_PIXELS = 262144
MIN_WORKSPACE_BYTES = FIXED_SCRATCH_BYTES + SCRATCH_BYTES_PER_PIXEL
MAX_WORKSPACE_BYTES = FIXED_SCRATCH_BYTES + SCRATCH_BYTES_PER_PIXEL * MAX_BATCH_PIXELS
DESTINATION_BYTES_PER_PIXEL = 3 * np.dtype(np.float64).itemsize


@dataclass(frozen=True)
class GamutWorkspacePlan:
    model: str
    pixel_count: int
    destination_bytes: int
    workspace_allowance_bytes: int
    scratch_bytes: int
    batch_pixels: int


def plan_gamut_workspace(pixel_count: int, workspace_bytes: int) -> GamutWorkspacePlan:
    """Plan scratch separately from the complete, owned destination buffer."""
    for label, value in (("pixel_count", pixel_count), ("workspace_bytes", workspace_bytes)):
        if type(value) is not int or value <= 0 or value > sys.maxsize:
            raise ValueError(f"{label} must be a positive platform-sized integer")
    if pixel_count > sys.maxsize // DESTINATION_BYTES_PER_PIXEL:
        raise ValueError("Gamut destination byte count exceeds the allocation range")
    if workspace_bytes < MIN_WORKSPACE_BYTES:
        raise ValueError(f"Gamut workspace requires at least {MIN_WORKSPACE_BYTES} bytes")
    batch_pixels = min(pixel_count, MAX_BATCH_PIXELS,
                       (workspace_bytes - FIXED_SCRATCH_BYTES) // SCRATCH_BYTES_PER_PIXEL)
    return GamutWorkspacePlan(
        MODEL_VERSION, pixel_count, pixel_count * DESTINATION_BYTES_PER_PIXEL,
        workspace_bytes, FIXED_SCRATCH_BYTES + batch_pixels * SCRATCH_BYTES_PER_PIXEL,
        batch_pixels,
    )


def compress_rgb_bounded(rgb, spec, *, output_color_space, workspace_bytes):
    """Return independent float64 storage; never normalize or mutate input.

    Reject unsupported layout/mode before allocation. Flattening a validated
    C-contiguous array is a view, so input size cannot add an implicit copy.
    Each upstream call receives at most the planned batch, in original order.
    """
    if output_color_space != "sRGB" or spec != OutputGamutCompressSpec():
        raise ValueError("Bounded gamut conversion requires the pinned CAM16-UCS sRGB recipe")
    if (type(rgb) is not np.ndarray or rgb.dtype != np.dtype(np.float64)
            or rgb.ndim < 1 or rgb.shape[-1] != 3 or not rgb.flags.c_contiguous):
        raise ValueError("Bounded gamut input must be a C-contiguous native float64 RGB array")
    plan = plan_gamut_workspace(rgb.size // 3, workspace_bytes)
    flat = rgb.reshape(-1, 3)
    destination = np.empty(rgb.shape, dtype=np.float64)
    output = destination.reshape(-1, 3)
    for first in range(0, plan.pixel_count, plan.batch_pixels):
        last = min(first + plan.batch_pixels, plan.pixel_count)
        output[first:last] = compress_rgb(
            flat[first:last], spec, output_color_space=output_color_space,
        )
    return destination
