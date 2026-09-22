"""Exact complete output against #343, with the JIT cache state made explicit."""
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile

cache_state = sys.argv[1]
cache = Path(os.environ["NUMBA_CACHE_DIR"])
if cache_state == "cold":
    assert not list(cache.iterdir()), "Cold reference requires an empty cache"
elif cache_state == "loaded":
    assert list(cache.rglob("*.nbc")), "Loaded reference requires compiled cache entries"
else:
    raise ValueError("Expected cold or loaded cache state")

sys.path.insert(0, "/opt/probe")
import numpy as np
from film import make_simulator, pixel_digest, render
from spektrafilm.utils.io import save_image_oiio

# Captured in both cache contexts from exact #343 image
# f921d83a626216598622b3801c7ec8c8b041f19e5b5bcd4f0a32cdc7b275f13e.
# Upstream cold compilation and cache loading differ before grain in the
# spatial DIR-coupler operation. Each context has one exact expected digest;
# the test neither accepts alternate hashes nor applies a numeric tolerance.
references = [
    (17, 19, "7ff1a228c01fa6e52024b3af79797444ac0d6ae5e3a3a49192616fcafbf60cce",
     "092b794c77bb504e67cf3b11cf28b91109857bcb81510946926eec86435fb792",
     "9030e30a29dc1e874e58a677a3302425f90bdb5ec815655abcd0ca6c00da7a03"),
    (128, 193, "894a8230d3e7eb76b9ee293a12d1b4cee63b4bf0f28254b4d0f98a6949ed5744",
     ("192dd4eceb4e5d35bd3d0268b9a1bad1dc581fc26ee35590f353ede4f3917b02"
      if cache_state == "cold" else
      "8c18cbdb565e30044eb78dd8358fb7b4ade9985efb6c12ea80487698c114ae40"),
     "b87e8bc4d89a23366f02d7b8fb7510177648c3e106731a818ec604bfb220a692"),
]
simulator, recipe = make_simulator()
with tempfile.TemporaryDirectory() as temp:
    for height, width, source_digest, output_digest, jpeg_digest in references:
        pixels = np.empty((height, width, 3), dtype=np.float32)
        pixels[:, :, 0] = np.linspace(.002, 1.4, width)[None, :]
        pixels[:, :, 1] = np.linspace(.05, 1, height)[:, None]
        pixels[:, :, 2] = .18
        assert pixel_digest(pixels) == source_digest
        result = render(simulator, pixels)
        assert pixel_digest(result) == output_digest
        filename = str(Path(temp) / "result.jpg")
        save_image_oiio(filename, result, color_space="sRGB", cctf_encoding=True)
        assert hashlib.sha256(Path(filename).read_bytes()).hexdigest() == jpeg_digest
        assert pixel_digest(pixels) == source_digest
        print(json.dumps({"event": "pipeline_reference", "cache_state": cache_state,
                          "shape": [height, width], "pixel_sha256": output_digest,
                          "jpeg_sha256": jpeg_digest}), flush=True)
