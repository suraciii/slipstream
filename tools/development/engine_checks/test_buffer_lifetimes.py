"""Reference behavior, actual ownership, and encoded-output regressions."""

from dataclasses import asdict
import gc
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import tracemalloc
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import weakref

import colour
import numpy as np
import OpenImageIO as oiio
from PIL import Image

sys.path.insert(0, "/opt/probe")
from film import make_simulator, pixel_digest, render
from spektrafilm.runtime.pipeline import SimulationPipeline
from spektrafilm.runtime.stages.scanning import ScanningStage
from spektrafilm.runtime.topology import Node, run_topology
from spektrafilm.utils import io
from spektrafilm.utils import bounded_output as bounded


def reference_walk(topology, inject, collect, image, on_fire=None):
    # Pinned 3bb2c2d dispatcher semantics, independent of the liveness planner.
    state = {inject: image}
    for node in topology:
        if all(k in state for k in node.reads):
            node.fire(state)
            if on_fire is not None:
                on_fire(node, 0)
            if collect in state:
                return state[collect]
    raise RuntimeError(f"no node path reaches tap {collect!r} from {inject!r}")


def cctf_allowance(batch):
    return bounded.CCTF_FIXED_BYTES + bounded.CCTF_BYTES_PER_PIXEL * batch


def jpeg_allowance(pixels):
    return bounded.JPEG_FIXED_BYTES + bounded.JPEG_BYTES_PER_PIXEL * pixels


class LifetimeTests(unittest.TestCase):
    def test_reference_walk_order_injection_collection_skips_and_overwrites(self):
        topology = [
            Node(("in",), ("x", "z"), lambda a: (a + 1, a + 10), "split"),
            Node(("missing", "x"), ("unused",), lambda *a: self.fail("unreachable"), "skip"),
            Node(("x",), ("y",), lambda a: a * 2, "left"),
            Node(("z",), ("x",), lambda a: a * 3, "overwrite"),
            Node(("x", "y"), ("out",), lambda a, b: a + b, "join"),
        ]
        for inject, collect in (("in", "out"), ("in", "x"), ("x", "y"),
                                ("in", "in"), ("missing", "missing"),
                                ("x", "out"), ("in", "absent")):
            observed = []
            for walk in (reference_walk, run_topology):
                events = []
                try:
                    value = walk(topology, inject, collect, 5,
                                 on_fire=lambda node, elapsed: events.append(node.label))
                    outcome = ("result", value)
                except RuntimeError as error:
                    outcome = ("error", str(error))
                observed.append((outcome, events))
            self.assertEqual(observed[0], observed[1], (inject, collect))

    def test_expired_arrays_are_freed_before_later_allocation(self):
        refs = {}
        def produce(name, increment):
            def run(value):
                if name == "last":
                    self.assertIsNone(refs["first"]())
                    self.assertIsNone(refs["second"]())
                result = value + increment
                refs[name] = weakref.ref(result)
                return result
            return run
        topology = [
            Node(("in",), ("a",), produce("first", 1)),
            Node(("a",), ("b",), produce("second", 2)),
            Node(("b",), ("c",), produce("third", 3)),
            Node(("c",), ("out",), produce("last", 4)),
        ]
        original = np.zeros((11, 13, 3))
        original.flags.writeable = False
        actual = run_topology(topology, "in", "out", original)
        np.testing.assert_array_equal(actual, np.full_like(original, 10))
        np.testing.assert_array_equal(original, 0)
        for name in ("first", "second", "third"):
            self.assertIsNone(refs[name]())
        self.assertIs(refs["last"](), actual)

    def test_branching_and_alias_views_keep_storage_until_last_reader(self):
        refs = {}
        def split(value):
            base = value + 1
            refs["base"] = weakref.ref(base)
            return base[:, :2], base[:, 2:]
        def right(value):
            self.assertIsNotNone(refs["base"]())
            return value.copy()
        def join(left, right):
            self.assertIsNone(refs["base"]())
            return np.concatenate((left, right), axis=1)
        topology = [Node(("in",), ("left", "right"), split),
                    Node(("left",), ("l2",), lambda x: x.copy()),
                    Node(("right",), ("r2",), right),
                    Node(("l2", "r2"), ("out",), join)]
        original = np.arange(15, dtype=float).reshape(3, 5)
        np.testing.assert_array_equal(run_topology(topology, "in", "out", original), original + 1)
        collected = run_topology(topology, "in", "left", original)
        self.assertIsNotNone(refs["base"]())
        np.testing.assert_array_equal(collected, (original + 1)[:, :2])
        del collected
        self.assertIsNone(refs["base"]())

    def test_overwritten_name_does_not_retain_old_value_during_new_writer(self):
        refs = {}
        def first(value):
            result = value + 1
            refs["old"] = weakref.ref(result)
            return result
        def overwrite(value):
            self.assertIsNone(refs["old"]())
            return value + 3
        nodes = [Node(("in",), ("x",), first),
                 Node(("x",), ("y",), lambda x: x + 2),
                 Node(("y",), ("x",), overwrite),
                 Node(("x",), ("out",), lambda x: x + 4)]
        np.testing.assert_array_equal(run_topology(nodes, "in", "out", np.zeros(3)), 10)

    def test_callback_error_propagates_and_stops_firing(self):
        events = []
        def fail(node, elapsed):
            events.append(node.label)
            raise LookupError("callback failure")
        nodes = [Node(("in",), ("middle",), lambda x: x + 1, "first"),
                 Node(("middle",), ("out",), lambda x: self.fail("ran after callback error"))]
        with self.assertRaisesRegex(LookupError, "callback failure"):
            run_topology(nodes, "in", "out", 0, on_fire=fail)
        self.assertEqual(events, ["first"])

    def test_preprocess_owns_one_float64_rgb_copy(self):
        source = np.arange(257 * 511 * 4, dtype=np.float32).reshape(257, 511, 4)
        source.flags.writeable = False
        pipeline = SimpleNamespace(_filming_stage=SimpleNamespace(auto_exposure=lambda x: x),
                                   _resize_service=SimpleNamespace(crop_and_rescale=lambda x: x))
        tracemalloc.start()
        actual = SimulationPipeline._preprocess(pipeline, source)
        _, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        self.assertLessEqual(peak, actual.nbytes + 65536)
        self.assertFalse(np.shares_memory(source, actual))
        self.assertTrue(actual.flags.owndata)
        np.testing.assert_array_equal(actual, source[:, :, :3])
        view = source[::-1, ::2]
        np.testing.assert_array_equal(SimulationPipeline._preprocess(pipeline, view), view[:, :, :3])

    def test_stage_handoff_reclaims_cycles_with_automatic_gc_disabled(self):
        refs = {}
        def first(value):
            result = value + 1
            refs["first"] = weakref.ref(result)
            cycle = [result]
            cycle.append(cycle)
            return result
        def second(value):
            return value + 1
        def last(value):
            self.assertIsNone(refs["first"]())
            result = value + 1
            refs["last"] = weakref.ref(result)
            return result
        enabled = gc.isenabled()
        gc.disable()
        try:
            result = run_topology([Node(("in",), ("a",), first),
                                   Node(("a",), ("b",), second),
                                   Node(("b",), ("out",), last)],
                                  "in", "out", np.zeros(3))
            self.assertIs(refs["last"](), result)
            np.testing.assert_array_equal(result, 3)
            self.assertFalse(gc.isenabled())
        finally:
            if enabled:
                gc.enable()

    def test_completed_pipeline_does_not_cache_frame_arrays(self):
        simulator, _ = make_simulator()
        pipeline = simulator._pipeline
        refs = []
        wrapped = []
        for node in pipeline._topology:
            def observe(*args, original=node.run):
                result = original(*args)
                refs.append(weakref.ref(result))
                return result
            wrapped.append(Node(node.reads, node.writes, observe, node.label))
        pipeline._topology = wrapped
        pixels = np.full((17, 19, 3), 0.18)
        enabled = gc.isenabled()
        gc.disable()
        try:
            result = render(simulator, pixels)
        finally:
            if enabled:
                gc.enable()
        self.assertTrue(all(ref() is None for ref in refs[:-1]))
        self.assertIs(refs[-1](), result)
        del result
        self.assertTrue(all(ref() is None for ref in refs))


class OutputTests(unittest.TestCase):
    def test_cctf_exact_thresholds_boundaries_and_owned_output(self):
        values = np.array([-0.02, 0, np.nextafter(.0031308, 0), .0031308,
                           np.nextafter(.0031308, 1), .18, 1, 2, 16], dtype=np.float64)
        for batch, count in ((1, 11), (7, 6), (7, 7), (7, 8), (7, 35),
                             (262144, 262143), (262144, 262144), (262144, 262145)):
            pixels = np.resize(values, (count, 3))
            original = pixels.copy()
            pixels.flags.writeable = False
            expected = colour.RGB_to_RGB(pixels, "sRGB", "sRGB", apply_cctf_decoding=False,
                                         apply_cctf_encoding=True)
            actual = bounded.encode_rgb_bounded(pixels, output_color_space="sRGB",
                                                workspace_bytes=cctf_allowance(batch))
            np.testing.assert_array_equal(actual, expected)
            np.testing.assert_array_equal(pixels, original)
            self.assertFalse(np.shares_memory(pixels, actual))
        pixels = np.random.default_rng(344).uniform(-.02, 2, (17, 19, 3))
        expected = colour.RGB_to_RGB(pixels, "sRGB", "sRGB", apply_cctf_decoding=False,
                                     apply_cctf_encoding=True)
        np.testing.assert_array_equal(
            bounded.encode_rgb_bounded(pixels, output_color_space="sRGB",
                                       workspace_bytes=cctf_allowance(7)), expected)
        stage = SimpleNamespace(_io=SimpleNamespace(output_cctf_encoding=False),
                                _output_gamut_workspace_bytes=cctf_allowance(7))
        self.assertIs(ScanningStage._apply_cctf_encoding(stage, pixels), pixels)
        for bad in (pixels[::2], pixels.astype(np.float32), np.empty((0, 3))):
            with self.assertRaises(ValueError):
                bounded.encode_rgb_bounded(bad, output_color_space="sRGB", workspace_bytes=cctf_allowance(1))
        with self.assertRaises(ValueError):
            bounded.plan_cctf_workspace(sys.maxsize, cctf_allowance(1))
        for space, allowance in (("ProPhoto RGB", cctf_allowance(1)), ("sRGB", cctf_allowance(1) - 1)):
            with patch.object(bounded.np, "empty", side_effect=AssertionError("allocated")):
                with self.assertRaises(ValueError):
                    bounded.encode_rgb_bounded(pixels, output_color_space=space, workspace_bytes=allowance)

    def test_quantization_exact_for_thresholds_tails_float32_and_strides(self):
        boundaries = np.arange(256, dtype=float) / 255
        values = np.concatenate(([-1, 0, 1, 2], boundaries,
                                 np.nextafter(boundaries, 0), np.nextafter(boundaries, 1)))
        for dtype in (np.float32, np.float64):
            source = np.resize(values.astype(dtype), (23, 17, 3))
            for pixels in (source, source[::-1, ::2], np.asfortranarray(source)):
                original = pixels.copy()
                pixels.flags.writeable = False
                plan = bounded.plan_jpeg_workspace(pixels, jpeg_allowance(pixels.shape[1] * 2))
                chunks = list(bounded.jpeg_row_batches(pixels, plan))
                self.assertEqual(chunks[-1][1], pixels.shape[0])
                self.assertTrue(all(last - first <= 2 for first, last, _ in chunks))
                actual = np.concatenate([data for _, _, data in chunks])
                expected = (np.clip(pixels, 0, 1) * 255.0).astype(np.uint8)
                np.testing.assert_array_equal(actual, expected)
                np.testing.assert_array_equal(pixels, original)
        with self.assertRaisesRegex(ValueError, "one full-width row"):
            bounded.plan_jpeg_workspace(source, jpeg_allowance(source.shape[1]) - 1)

    def test_jpeg_scanlines_match_original_encoded_bytes_and_icc(self):
        with tempfile.TemporaryDirectory() as temp:
            for dtype in (np.float32, np.float64):
                pixels = np.random.default_rng(344).uniform(-.02, 1.2, (29, 17, 3)).astype(dtype)
                for encoded in (False, True):
                    reference = str(Path(temp) / "reference.jpg")
                    candidate = str(Path(temp) / "candidate.jpg")
                    expected = (np.clip(pixels, 0, 1) * 255.0).astype(np.uint8)
                    spec = oiio.ImageSpec(17, 29, 3, oiio.UINT8)
                    profile = io._load_icc_profile("sRGB", encoded)
                    icc = np.frombuffer(profile, dtype=np.uint8)
                    spec.attribute("ICCProfile", oiio.TypeDesc(f"uint8[{len(icc)}]"), icc)
                    writer = oiio.ImageOutput.create(reference)
                    self.assertTrue(writer.open(reference, spec))
                    self.assertTrue(writer.write_image(expected))
                    self.assertTrue(writer.close())
                    io.save_image_oiio(candidate, pixels, color_space="sRGB", cctf_encoding=encoded,
                                       jpeg_workspace_bytes=jpeg_allowance(17 * 2))
                    self.assertEqual(Path(reference).read_bytes(), Path(candidate).read_bytes())
                    with Image.open(candidate) as image:
                        self.assertEqual(image.size, (17, 29))
                        self.assertEqual(image.info["icc_profile"], profile)

    def test_jpeg_errors_and_cancellation_close_writer_and_invalid_budget_never_opens(self):
        pixels = np.ones((5, 7, 3), dtype=np.float64)
        factory = Mock()
        replacement = SimpleNamespace(create=factory)
        with patch.object(io.oiio, "ImageOutput", replacement):
            with self.assertRaises(ValueError):
                io.save_image_oiio("invalid.jpg", pixels, jpeg_workspace_bytes=jpeg_allowance(7) - 1)
            factory.assert_not_called()
            factory.return_value = None
            with self.assertRaises(IOError):
                io.save_image_oiio("create.jpg", pixels)
            for stage in ("open", "write_scanlines", "close", "cancel"):
                writer = Mock()
                writer.open.return_value = True
                writer.write_scanlines.return_value = True
                writer.close.return_value = True
                writer.geterror.return_value = "controlled writer error"
                if stage == "cancel":
                    writer.write_scanlines.side_effect = KeyboardInterrupt("cancelled")
                else:
                    getattr(writer, stage).return_value = False
                factory.return_value = writer
                with self.assertRaises(KeyboardInterrupt if stage == "cancel" else IOError):
                    io.save_image_oiio("failure.jpg", pixels, jpeg_workspace_bytes=jpeg_allowance(7))
                writer.close.assert_called_once()

    def test_decoded_samples_survive_reader_close_collection_and_file_removal(self):
        with tempfile.TemporaryDirectory() as temp:
            filename = str(Path(temp) / "source.tif")
            pixels = np.random.default_rng(344).random((11, 13, 3)).astype(np.float32)
            io.save_image_oiio(filename, pixels, bit_depth=32)
            result = io.load_image_oiio(filename)
            Path(filename).unlink()
            gc.collect()
            np.testing.assert_array_equal(result, pixels)
            result[0, 0, 0] = -1
            self.assertNotEqual(result[0, 0, 0], pixels[0, 0, 0])

    def test_bounded_observation_preserves_c_order_digest_and_finite_semantics(self):
        data = np.arange(262145 * 3, dtype=np.float64).reshape(5, 52429, 3)
        for values in (data, data[:, ::2], data[::-1], data.transpose(1, 0, 2),
                       data.astype(">f8"), np.empty((0, 3))):
            expected = hashlib.sha256(values.tobytes()).hexdigest()
            self.assertEqual(pixel_digest(values), expected)
            for chunk in bounded.c_order_chunks(values, chunk_samples=7):
                self.assertLessEqual(chunk.size, 7)
                self.assertTrue(chunk.flags.c_contiguous)
            self.assertEqual(bounded.samples_are_finite(values), bool(np.isfinite(values).all()))
        for bad in (np.nan, np.inf, -np.inf):
            data[-1, -1, -1] = bad
            self.assertFalse(bounded.samples_are_finite(data[:, ::-1]))
        data.fill(.18)
        tracemalloc.start()
        pixel_digest(data[:, ::-1])
        bounded.samples_are_finite(data[:, ::-1])
        _, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        self.assertLess(peak, 2 * bounded.OBSERVATION_CHUNK_SAMPLES * 8)

    def test_new_numeric_scratch_models_cover_max_batches(self):
        pixels = np.random.default_rng(344).uniform(-.02, 2, (512, 512, 3))
        cctf_plan = bounded.plan_cctf_workspace(262144, cctf_allowance(262144))
        tracemalloc.start()
        bounded.encode_rgb_bounded(pixels, output_color_space="sRGB",
                                    workspace_bytes=cctf_plan.workspace_allowance_bytes)
        _, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        self.assertLessEqual(peak, cctf_plan.destination_bytes + cctf_plan.scratch_bytes)
        print(json.dumps({"event": "cctf_scratch", "plan": asdict(cctf_plan), "tracked_peak": peak}), flush=True)
        for source in (pixels, pixels[::-1]):
            plan = bounded.plan_jpeg_workspace(source)
            tracemalloc.start()
            for _, _, batch in bounded.jpeg_row_batches(source, plan):
                self.assertEqual(batch.dtype, np.dtype(np.uint8))
            _, peak = tracemalloc.get_traced_memory()
            tracemalloc.stop()
            self.assertLessEqual(peak, plan.scratch_bytes)
            print(json.dumps({"event": "jpeg_numeric_scratch", "plan": asdict(plan), "tracked_peak": peak}), flush=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
