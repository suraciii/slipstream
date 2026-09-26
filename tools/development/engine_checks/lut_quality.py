"""Deterministic corpus and metrics for LUT versus direct Film comparison."""

import numpy as np


GEOMETRY = (128, 193)


def comparison_corpus():
    """Return small, fixed full-effect inputs with different signal structure."""
    height, width = GEOMETRY
    y, x = np.indices((height, width), dtype=np.float64)
    horizontal = x / (width - 1)
    vertical = y / (height - 1)
    neutral = np.full((height, width, 3), 0.18, dtype=np.float64)

    structured = np.empty((height, width, 3), dtype=np.float64)
    structured[:, :, 0] = -0.12 + 1.45 * horizontal
    structured[:, :, 1] = 0.02 + 0.92 * vertical
    structured[:, :, 2] = 0.08 + 0.68 * (1.0 - horizontal * vertical)

    generator = np.random.default_rng(338)
    textured = generator.uniform(-0.08, 1.2, (height, width, 3)).astype(np.float64)
    return (
        ("neutral", neutral),
        ("structured-range", structured),
        ("textured-range", textured),
    )


def nearest_rank(values, fraction):
    """Return a deterministic nearest-rank percentile from a one-dimensional array."""
    values = np.sort(np.asarray(values, dtype=np.float64).reshape(-1))
    if values.size == 0 or not 0.0 <= fraction <= 1.0:
        raise ValueError("percentile requires nonempty values and a fraction in [0, 1]")
    rank = max(1, int(np.ceil(fraction * values.size)))
    return float(values[rank - 1])


def summarize_difference(reference, candidate):
    """Summarize pointwise absolute RGB differences without declaring acceptance."""
    reference = np.asarray(reference)
    candidate = np.asarray(candidate)
    if reference.shape != candidate.shape or reference.ndim != 3 or reference.shape[-1] != 3:
        raise ValueError("Film outputs must have matching H x W x 3 shapes")
    difference = np.abs(reference.astype(np.float64) - candidate.astype(np.float64))
    flattened = difference.reshape(-1)
    return {
        "samples": int(flattened.size),
        "mean_abs_channel": float(np.mean(flattened)),
        "p95_abs_channel": nearest_rank(flattened, 0.95),
        "max_abs_channel": float(np.max(flattened)),
        "nonzero_channel_count": int(np.count_nonzero(flattened)),
    }
