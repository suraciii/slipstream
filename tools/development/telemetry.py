"""Small process observations used by the Film qualification probe."""

from __future__ import annotations

import os
from pathlib import Path
import stat
import threading
import time


_SAMPLE_SECONDS = 0.005


def process_thread_count() -> int:
    """Return the current native thread count, with a portable test fallback."""
    task_root = Path("/proc/self/task")
    try:
        return sum(1 for entry in task_root.iterdir() if entry.is_dir())
    except OSError:
        return threading.active_count()


def workspace_file_bytes(root: Path) -> int:
    """Sum regular-file sizes below *root* without following symlinks."""
    total = 0
    for directory, _subdirectories, files in os.walk(root, followlinks=False):
        for name in files:
            try:
                facts = os.stat(Path(directory) / name, follow_symlinks=False)
            except OSError:
                continue
            if stat.S_ISREG(facts.st_mode):
                total += facts.st_size
    return total


class RenderTelemetry:
    """Sample process threads while one synchronous render is running.

    The sampler is one extra thread; the reported count subtracts that known
    observer so the result describes the render process rather than the probe's
    measurement machinery. Sampling is diagnostic and does not replace cgroup
    accounting.
    """

    def __init__(self) -> None:
        self._stop = threading.Event()
        self._maximum = 0
        self._maximum_lock = threading.Lock()
        self._sampler: threading.Thread | None = None

    def _record_maximum(self, value: int) -> None:
        with self._maximum_lock:
            self._maximum = max(self._maximum, value)

    def __enter__(self) -> "RenderTelemetry":
        # Reserve one count for the sampler before it starts, so an
        # exceptionally short render cannot under-report the baseline.
        self._record_maximum(process_thread_count() + 1)
        self._sampler = threading.Thread(target=self._sample, name="render-telemetry")
        self._sampler.daemon = True
        self._sampler.start()
        return self

    def _sample(self) -> None:
        while not self._stop.is_set():
            self._record_maximum(process_thread_count())
            self._stop.wait(_SAMPLE_SECONDS)

    def __exit__(self, _exception_type, _exception, _traceback) -> None:
        self._stop.set()
        # The sampler is still alive until join(), so this snapshot includes
        # its known one-thread overhead.
        self._record_maximum(process_thread_count())
        assert self._sampler is not None
        self._sampler.join()

    @property
    def observed_threads(self) -> int:
        """Maximum process threads observed, excluding this sampler thread."""
        with self._maximum_lock:
            maximum = self._maximum
        return max(1, maximum - 1)
