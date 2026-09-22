"""Observe first-process JIT lifetime without relying on automatic collection."""
import gc
import json
import os
import sys
import tempfile
import time
import weakref

# A new directory proves this process cannot reuse a previous compiled cache.
os.environ["NUMBA_CACHE_DIR"] = tempfile.mkdtemp(prefix="cold-lifetime-", dir="/work")
sys.path.insert(0, "/opt/probe")
import numpy as np
from film import make_simulator, pixel_digest, render
from spektrafilm.runtime.topology import Node

simulator, _ = make_simulator()
refs = []
nodes = []
for node in simulator._pipeline._topology:
    def observe(*args, original=node.run, label=node.label):
        for previous_label, ref in refs[:-1]:
            assert ref() is None, f"{previous_label} retained before {label}"
        result = original(*args)
        refs.append((label, weakref.ref(result)))
        return result
    nodes.append(Node(node.reads, node.writes, observe, node.label))
simulator._pipeline._topology = nodes
pixels = np.full((17, 19, 3), .18, dtype=np.float32)
enabled = gc.isenabled()
gc.disable()
try:
    expected = None
    for iteration in range(2):
        refs.clear()
        start = time.monotonic()
        result = render(simulator, pixels)
        elapsed = time.monotonic() - start
        assert all(ref() is None for _, ref in refs[:-1])
        digest = pixel_digest(result)
        assert expected is None or expected == digest
        expected = digest
        assert not gc.isenabled(), "Production changed process-global GC policy"
        print(json.dumps({"event": "cold_lifetime", "iteration": iteration,
                          "seconds": elapsed, "pixel_sha256": digest,
                          "timings": simulator.get_timings()}), flush=True)
        del result
        assert all(ref() is None for _, ref in refs), "Completed frame retained by runtime"
finally:
    if enabled:
        gc.enable()
