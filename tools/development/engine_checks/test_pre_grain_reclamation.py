"""Deterministic ownership and branch checks for the interpolation handoff."""

import gc
import unittest
from unittest.mock import patch
import weakref

import numpy as np
import spektrafilm.model.grain as model
from spektrafilm.runtime.params_schema import GrainParams


class ReclamationTests(unittest.TestCase):
    def call(self, density, grain=None, **kwargs):
        return model.apply_grain(
            density, 10.0, grain or GrainParams(), np.ones((5, 3)),
            np.ones((5, 3, 3)), kwargs.pop("profile_type", "negative"), **kwargs,
        )

    def assert_cycle_handoff(self, profile_type):
        original = np.arange(36, dtype=np.float64).reshape(2, 6, 3)
        density = original[:, ::2]
        density.flags.writeable = False
        unchanged = original.copy()
        refs = {}
        rng_before = np.random.get_state()
        gc_before = gc.isenabled(), gc.get_threshold()

        def interpolate(actual, curves, layer_curves, *, positive_film):
            self.assertIs(actual, density)
            self.assertEqual(positive_film, profile_type == "positive")
            owner = np.full((2, 3, 3, 4), 0.5)
            layers = owner[..., :3]
            scratch = np.full((2, 3, 3), 17.0)
            cycle = [scratch, density]
            cycle.append(cycle)
            refs.update(scratch=weakref.ref(scratch), layers=weakref.ref(layers),
                        owner=weakref.ref(owner))
            return layers

        def particles(layers, **kwargs):
            self.assertIsNone(refs["scratch"](), "dead scratch survived handoff")
            self.assertIs(refs["layers"](), layers)
            self.assertTrue(np.shares_memory(layers, refs["owner"]()))
            np.testing.assert_array_equal(layers, 0.5)
            np.testing.assert_array_equal(original, unchanged)
            self.assertFalse(gc.isenabled())
            self.assertEqual(gc.get_threshold(), gc_before[1])
            return density

        gc.disable()
        try:
            with patch.object(model, "interp_density_cmy_layers", interpolate), \
                 patch.object(model, "apply_grain_to_density_layers", particles):
                result = self.call(density, profile_type=profile_type)
            self.assertIs(result, density)
            self.assertIsNone(refs["layers"]())
            self.assertIsNone(refs["owner"]())
            np.testing.assert_array_equal(original, unchanged)
            rng_after = np.random.get_state()
            self.assertEqual(rng_before[0], rng_after[0])
            np.testing.assert_array_equal(rng_before[1], rng_after[1])
            self.assertEqual(rng_before[2:], rng_after[2:])
        finally:
            if gc_before[0]:
                gc.enable()

    def test_collects_cycles_and_preserves_live_aliases_in_both_branches(self):
        for profile_type in ("negative", "positive"):
            with self.subTest(profile_type=profile_type):
                self.assert_cycle_handoff(profile_type)

    def test_lifetime_assertion_detects_an_absent_production_collection(self):
        with patch.object(model.gc, "collect", return_value=0):
            with self.assertRaisesRegex(AssertionError, "dead scratch survived"):
                self.assert_cycle_handoff("negative")

    def test_inactive_and_bypassed_grain_return_borrowed_input(self):
        density = np.zeros((2, 3, 3))
        for active, bypass in ((False, False), (True, True)):
            with self.subTest(active=active, bypass=bypass), \
                 patch.object(model, "interp_density_cmy_layers") as interpolation, \
                 patch.object(model, "apply_grain_to_density_layers") as particles, \
                 patch.object(model.gc, "collect") as collect:
                grain = GrainParams()
                grain.active = active
                self.assertIs(self.call(density, grain, bypass_grain=bypass), density)
                interpolation.assert_not_called()
                particles.assert_not_called()
                collect.assert_not_called()

    def test_non_sublayer_grain_keeps_its_consumer_without_collection(self):
        density = np.zeros((2, 3, 3))
        grain = GrainParams()
        grain.sublayers_active = False
        expected = density + 1
        with patch.object(model, "interp_density_cmy_layers") as interpolation, \
             patch.object(model, "apply_grain_to_density", return_value=expected) as particles, \
             patch.object(model.gc, "collect") as collect:
            self.assertIs(self.call(density, grain), expected)
            self.assertIs(particles.call_args.args[0], density)
            interpolation.assert_not_called()
            collect.assert_not_called()

    def test_interpolation_error_precedes_collection_and_sampling(self):
        failure = RuntimeError("interpolation failed")
        with patch.object(model, "interp_density_cmy_layers", side_effect=failure), \
             patch.object(model, "apply_grain_to_density_layers") as particles, \
             patch.object(model.gc, "collect") as collect:
            with self.assertRaises(RuntimeError) as raised:
                self.call(np.zeros((2, 3, 3)))
            self.assertIs(raised.exception, failure)
            particles.assert_not_called()
            collect.assert_not_called()

    def test_particle_error_follows_one_completed_handoff(self):
        events = []
        layers = np.ones((2, 3, 3, 3))
        failure = RuntimeError("particle failed")
        def collect():
            events.append("collect")
        def particles(actual, **kwargs):
            self.assertIs(actual, layers)
            events.append("particles")
            raise failure
        with patch.object(model, "interp_density_cmy_layers", return_value=layers), \
             patch.object(model, "apply_grain_to_density_layers", particles), \
             patch.object(model.gc, "collect", collect):
            with self.assertRaises(RuntimeError) as raised:
                self.call(np.zeros((2, 3, 3)))
            self.assertIs(raised.exception, failure)
            self.assertEqual(events, ["collect", "particles"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
