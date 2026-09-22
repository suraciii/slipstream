"""Fresh-process numerical output scratch; native encoder memory is separate."""
from dataclasses import asdict
import json
import resource
import sys
import tracemalloc
import numpy as np
from spektrafilm.utils import bounded_output as bounded

operation, size, layout = sys.argv[1:]
count = int(size)
if operation == "cctf":
    pixels = np.random.default_rng(344).uniform(-.02, 2, (count, 3))
    plan = bounded.plan_cctf_workspace(count, bounded.CCTF_FIXED_BYTES + count * bounded.CCTF_BYTES_PER_PIXEL)
else:
    side = 1 if count == 1 else 512
    pixels = np.random.default_rng(344).uniform(-.02, 2, (side, side, 3))
    if layout == "strided":
        pixels = pixels[::-1, ::-1]
    plan = bounded.plan_jpeg_workspace(pixels, bounded.JPEG_FIXED_BYTES + count * bounded.JPEG_BYTES_PER_PIXEL)
tracemalloc.start()
if operation == "cctf":
    result = bounded.encode_rgb_bounded(pixels, output_color_space="sRGB", workspace_bytes=plan.workspace_allowance_bytes)
else:
    for _, _, result in bounded.jpeg_row_batches(pixels, plan):
        pass
_, peak = tracemalloc.get_traced_memory()
tracemalloc.stop()
assert peak <= plan.destination_bytes + plan.scratch_bytes
assert np.isfinite(result).all()
print(json.dumps({"event": "fresh_output_scratch", "operation": operation, "layout": layout,
                  "plan": asdict(plan), "tracked_peak_bytes": peak,
                  "process_peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}), flush=True)
