import sys
from pathlib import Path
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).with_name("engine_checks")))
from lut_quality import comparison_corpus, summarize_difference  # noqa: E402


class LutQualityTests(unittest.TestCase):
    def test_corpus_is_fixed_and_covers_signal_ranges(self):
        first = comparison_corpus()
        second = comparison_corpus()
        self.assertEqual([name for name, _ in first],
                         ["neutral", "structured-range", "textured-range"])
        for (_, left), (_, right) in zip(first, second):
            np.testing.assert_array_equal(left, right)
            self.assertEqual(left.shape, (128, 193, 3))
            self.assertTrue(np.isfinite(left).all())
        self.assertLess(first[1][1].min(), 0)
        self.assertGreater(first[1][1].max(), 1)

    def test_summary_uses_nearest_rank_and_preserves_zero_case(self):
        reference = np.zeros((1, 2, 3), dtype=np.float32)
        candidate = np.array([[[0, 1, 2], [3, 4, 5]]], dtype=np.float32)
        summary = summarize_difference(reference, candidate)
        self.assertEqual(summary["samples"], 6)
        self.assertEqual(summary["mean_abs_channel"], 2.5)
        self.assertEqual(summary["p95_abs_channel"], 5.0)
        self.assertEqual(summary["max_abs_channel"], 5.0)
        self.assertEqual(summary["nonzero_channel_count"], 5)
        zero = summarize_difference(reference, reference)
        self.assertEqual(zero["max_abs_channel"], 0.0)
        self.assertEqual(zero["nonzero_channel_count"], 0)

    def test_summary_rejects_wrong_shapes(self):
        with self.assertRaises(ValueError):
            summarize_difference(np.zeros((2, 2, 3)), np.zeros((2, 2, 1)))


if __name__ == "__main__":
    unittest.main()
