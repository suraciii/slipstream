"""Real pinned-engine checks. Run inside the qualification image, not on host."""

from dataclasses import asdict
import json
from pathlib import Path
import sys
import tracemalloc
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, "/opt/probe")
from film import make_simulator, render
from spektrafilm import Simulator, digest_params, init_params
from spektrafilm.utils import bounded_gamut as bounded
from spektrafilm.utils import gamut_compression as upstream


def allowance(batch):
    return bounded.FIXED_SCRATCH_BYTES + bounded.SCRATCH_BYTES_PER_PIXEL * batch


def sample(count):
    pixels = np.random.default_rng(343).uniform(-0.02, 4.0, (count, 3))
    landmarks = np.array([
        [0, 0, 0], [0.18, 0.18, 0.18], [1, 1, 1], [-0.02, -0.01, -0.005],
        [1, 0, 0], [0, 1, 0], [0, 0, 1], [16, 8, 32], [-1e-6, 1e-9, 1e-3],
    ])
    pixels[:min(count, len(landmarks))] = landmarks[:count]
    return pixels


class BoundedGamutTests(unittest.TestCase):
    def test_budget_arithmetic_and_rejection_before_destination(self):
        for invalid in (-1, 0, True, 1.0, sys.maxsize + 1):
            with self.assertRaises(ValueError):
                bounded.plan_gamut_workspace(1, invalid)
            with self.assertRaises(ValueError):
                bounded.plan_gamut_workspace(invalid, allowance(1))
        with self.assertRaises(ValueError):
            bounded.plan_gamut_workspace(sys.maxsize, allowance(1))
        pixels = sample(3)
        for workspace in (allowance(1) - 1, 1):
            with patch.object(bounded.np, "empty", side_effect=AssertionError("allocated")):
                with self.assertRaisesRegex(ValueError, "requires at least"):
                    bounded.compress_rgb_bounded(pixels, upstream.OutputGamutCompressSpec(),
                                                 output_color_space="sRGB", workspace_bytes=workspace)
        plan = bounded.plan_gamut_workspace(31, allowance(7) + 1023)
        self.assertEqual(plan.batch_pixels, 7)
        self.assertEqual(plan.destination_bytes, 31 * 24)
        self.assertEqual(plan.scratch_bytes, allowance(7))
        self.assertEqual(bounded.plan_gamut_workspace(3, allowance(7)).batch_pixels, 3)
        self.assertEqual(bounded.plan_gamut_workspace(10**6, sys.maxsize).batch_pixels,
                         bounded.MAX_BATCH_PIXELS)

    def test_rejects_layout_dtype_empty_and_unqualified_modes(self):
        original = sample(15).reshape(3, 5, 3)
        bad_inputs = [original[:, ::2], original[::-1], original.transpose(1, 0, 2),
                      np.asfortranarray(original), original.astype(np.float32),
                      original.astype(">f8"), np.empty((0, 3)), np.zeros((3, 2)), [0., 0., 0.]]
        for value in bad_inputs:
            with self.subTest(shape=getattr(value, "shape", None)):
                with patch.object(bounded.np, "empty", side_effect=AssertionError("allocated")):
                    with self.assertRaises(ValueError):
                        bounded.compress_rgb_bounded(value, upstream.OutputGamutCompressSpec(),
                                                     output_color_space="sRGB", workspace_bytes=allowance(7))
        for spec, space in ((upstream.OutputGamutCompressSpec(algorithm="off"), "sRGB"),
                            (upstream.OutputGamutCompressSpec(), "ProPhoto RGB"),
                            (upstream.OutputGamutCompressSpec(knee=(0.5, 1, 2)), "sRGB")):
            with patch.object(bounded.np, "empty", side_effect=AssertionError("allocated")):
                with self.assertRaises(ValueError):
                    bounded.compress_rgb_bounded(original, spec, output_color_space=space,
                                                 workspace_bytes=allowance(7))

    def test_exact_pointwise_pixels_boundaries_readonly_and_rng(self):
        spec = upstream.OutputGamutCompressSpec()
        for batch, count in ((1, 11), (7, 6), (7, 7), (7, 8), (7, 35), (7, 37),
                             (bounded.MAX_BATCH_PIXELS, bounded.MAX_BATCH_PIXELS - 1),
                             (bounded.MAX_BATCH_PIXELS, bounded.MAX_BATCH_PIXELS),
                             (bounded.MAX_BATCH_PIXELS, bounded.MAX_BATCH_PIXELS + 1)):
            with self.subTest(batch=batch, count=count):
                pixels = sample(count)
                if count == 35:
                    pixels = pixels.reshape(5, 7, 3)
                original = pixels.copy()
                pixels.flags.writeable = False
                expected = upstream.compress_rgb(pixels, spec, output_color_space="sRGB")
                state = np.random.get_state()
                with patch.object(bounded, "compress_rgb", wraps=upstream.compress_rgb) as calls:
                    actual = bounded.compress_rgb_bounded(pixels, spec, output_color_space="sRGB",
                                                          workspace_bytes=allowance(batch))
                np.testing.assert_array_equal(actual, expected)
                np.testing.assert_array_equal(pixels, original)
                self.assertFalse(np.shares_memory(actual, pixels))
                self.assertTrue(actual.flags.c_contiguous and actual.flags.owndata)
                for call in calls.call_args_list:
                    self.assertLessEqual(call.args[0].shape[0], batch)
                    self.assertTrue(np.shares_memory(call.args[0], pixels))
                after = np.random.get_state()
                np.testing.assert_array_equal(state[1], after[1])
                self.assertEqual(state[2:], after[2:])

    def test_full_seeded_pipeline_exact_across_budgets_and_parameter_update(self):
        # Use the identical complete recipe, with only the explicit memory policy changed.
        bounded_simulator, manifest = make_simulator(gamut_workspace_bytes=allowance(1))
        params = bounded_simulator._pipeline._params
        reference = Simulator(params)
        pixels = sample(35).reshape(5, 7, 3)
        pixels = np.clip(pixels, 0.002, 1.4)
        original = pixels.copy()
        expected = render(reference, pixels)
        self.assertTrue(params.film_render.grain.active)
        self.assertTrue(params.film_render.halation.active)
        self.assertTrue(params.print_render.glare.active)
        self.assertFalse(params.debug.deactivate_spatial_effects)
        self.assertFalse(params.debug.deactivate_stochastic_effects)
        for batch in (1, 7, bounded.MAX_BATCH_PIXELS):
            simulator, recipe = make_simulator(gamut_workspace_bytes=allowance(batch))
            actual = render(simulator, pixels)
            np.testing.assert_array_equal(actual, expected)
            np.testing.assert_array_equal(pixels, original)
            simulator.update_params(params)
            self.assertEqual(simulator._pipeline._output_gamut_workspace_bytes, allowance(batch))
            np.testing.assert_array_equal(render(simulator, pixels), expected)
            self.assertTrue(recipe["processing_bundle"]["patches"])
        # A second geometry and different content exercise seeded grain spatial scale.
        pixels = np.full((17, 19, 3), 0.18)
        np.testing.assert_array_equal(render(bounded_simulator, pixels), render(reference, pixels))
        pixels = np.clip(sample(128 * 193).reshape(128, 193, 3), 0.002, 1.4)
        expected = render(reference, pixels)
        for batch in (127, bounded.MAX_BATCH_PIXELS):
            simulator, _ = make_simulator(gamut_workspace_bytes=allowance(batch))
            np.testing.assert_array_equal(render(simulator, pixels), expected)

    def test_cold_transform_scratch_envelope(self):
        # NumPy's allocator participates in tracemalloc. This checks algorithm
        # workspace, not process RSS, native runtime headroom, or cgroup admission.
        for count in (1, 65536, bounded.MAX_BATCH_PIXELS):
            pixels = sample(count)
            upstream._OUTPUT_CMAX_CACHE.clear()
            plan = bounded.plan_gamut_workspace(count, allowance(count))
            tracemalloc.start()
            try:
                bounded.compress_rgb_bounded(pixels, upstream.OutputGamutCompressSpec(),
                                             output_color_space="sRGB", workspace_bytes=allowance(count))
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            self.assertLessEqual(peak, plan.destination_bytes + plan.scratch_bytes)
            print(json.dumps({"event": "transform_scratch", "plan": asdict(plan),
                              "tracked_peak_bytes": peak}), flush=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
