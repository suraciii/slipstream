"""Deterministic tests for the LUT qualification corpus and summaries."""

import unittest

import numpy as np

from lut_quality import comparison_corpus, summarize_difference


class LutQualityTests(unittest.TestCase):
    def test_corpus_is_fixed_full_effect_and_varied(self):
        first = comparison_corpus()
        second = comparison_corpus()

        self.assertEqual([name for name, _ in first], [
            "neutral", "structured-range", "textured-range",
        ])
        self.assertEqual([name for name, _ in first], [name for name, _ in second])
        for (_, pixels), (_, repeated) in zip(first, second):
            np.testing.assert_array_equal(pixels, repeated)
            self.assertEqual(pixels.shape, (128, 193, 3))
            self.assertTrue(np.isfinite(pixels).all())
        self.assertLess(first[1][1].min(), 0.0)
        self.assertGreater(first[1][1].max(), 1.0)

    def test_summary_reports_nearest_rank_and_channel_counts(self):
        reference = np.zeros((1, 2, 3), dtype=np.float64)
        candidate = np.array([[[0.0, 1.0, 2.0], [3.0, 4.0, 5.0]]])

        self.assertEqual(summarize_difference(reference, candidate), {
            "samples": 6,
            "mean_abs_channel": 2.5,
            "p95_abs_channel": 5.0,
            "max_abs_channel": 5.0,
            "nonzero_channel_count": 5,
        })

    def test_summary_rejects_non_rgb_shapes(self):
        with self.assertRaises(ValueError):
            summarize_difference(np.zeros((2, 3)), np.zeros((2, 3)))
        with self.assertRaises(ValueError):
            summarize_difference(np.zeros((2, 3, 3)), np.zeros((2, 3, 4)))


if __name__ == "__main__":
    unittest.main(verbosity=2)
