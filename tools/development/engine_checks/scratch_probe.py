"""Fresh-process transform allocation evidence; no complete-stage budget claim."""

from dataclasses import asdict
import json
import resource
import sys
import tracemalloc

import numpy as np

from spektrafilm.utils.bounded_gamut import (
    FIXED_SCRATCH_BYTES, SCRATCH_BYTES_PER_PIXEL, compress_rgb_bounded,
    plan_gamut_workspace,
)
from spektrafilm.utils.gamut_compression import OutputGamutCompressSpec

count = int(sys.argv[1])
case = sys.argv[2]
if case == "mixed":
    pixels = np.random.default_rng(343).uniform(-0.02, 4, (count, 3))
elif case == "neutral":
    pixels = np.full((count, 3), 0.18)
else:
    raise SystemExit("unknown scratch probe content")
plan = plan_gamut_workspace(count, FIXED_SCRATCH_BYTES + count * SCRATCH_BYTES_PER_PIXEL)
tracemalloc.start()
result = compress_rgb_bounded(pixels, OutputGamutCompressSpec(),
                              output_color_space="sRGB", workspace_bytes=plan.workspace_allowance_bytes)
_, peak = tracemalloc.get_traced_memory()
tracemalloc.stop()
assert peak <= plan.destination_bytes + plan.scratch_bytes
assert np.isfinite(result).all()
print(json.dumps({"event": "fresh_transform_scratch", "content": case,
                  "plan": asdict(plan), "tracked_peak_bytes": peak,
                  "process_peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}), flush=True)
