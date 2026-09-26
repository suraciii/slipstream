import tempfile
from pathlib import Path
import threading
import time
import unittest
from unittest.mock import patch

from telemetry import RenderTelemetry, process_thread_count, workspace_file_bytes


class TelemetryTests(unittest.TestCase):
    def test_process_thread_count_is_positive(self):
        self.assertGreaterEqual(process_thread_count(), 1)

    def test_workspace_bytes_counts_regular_files_without_following_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "nested").mkdir()
            (root / "one").write_bytes(b"123")
            (root / "nested" / "two").write_bytes(b"45")
            outside = root / "outside"
            outside.write_bytes(b"not included")
            (root / "link").symlink_to(outside)
            self.assertEqual(workspace_file_bytes(root), 5 + len(b"not included"))
            self.assertEqual(workspace_file_bytes(root / "nested"), 2)

    def test_render_telemetry_observes_a_render_process_thread(self):
        baseline = process_thread_count()
        started = threading.Event()
        with patch("telemetry._SAMPLE_SECONDS", 0.0001):
            with RenderTelemetry() as telemetry:
                worker = threading.Thread(
                    target=lambda: (started.set(), time.sleep(0.05))
                )
                worker.start()
                self.assertTrue(started.wait(1))
                worker.join()
        self.assertGreaterEqual(telemetry.observed_threads, baseline + 1)


if __name__ == "__main__":
    unittest.main()
