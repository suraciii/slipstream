from dataclasses import asdict
from contextlib import ExitStack
import copy
import errno
import hashlib
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
import weakref
from unittest.mock import patch

import numpy as np

import adapter
from contract import ContractError
from test_contract import grant
from spektrafilm.utils.bounded_gamut import plan_gamut_workspace
from spektrafilm.utils.bounded_output import plan_cctf_workspace, plan_jpeg_workspace


class AdapterTests(unittest.TestCase):
    def test_packaged_recipe_matches_real_simulator_manifest(self):
        adapter.sys.path.insert(0, "/opt/probe")
        from film import make_simulator
        from contract import NUMERICAL_BUNDLE

        _, recipe = make_simulator()
        packaged_bundle = Path("/opt/processing-bundle.json").read_bytes()
        self.assertEqual(hashlib.sha256(packaged_bundle).hexdigest(), NUMERICAL_BUNDLE)
        self.assertEqual(recipe["processing_bundle"], json.loads(packaged_bundle))
        adapter.check_recipe(recipe)

        changed = copy.deepcopy(recipe)
        changed["camera"]["exposure_compensation_ev"] = 1.0
        with self.assertRaises(ContractError):
            adapter.check_recipe(changed)

    def test_opaque_codec_error_requires_real_storage_exhaustion_evidence(self):
        adapter.sys.path.insert(0, "/opt/probe")
        import film
        from spektrafilm.utils import io

        value = grant()
        reference = value["fixture"]["reference"]
        for blocks, inodes, expected_errno in ((0, 1, errno.ENOSPC), (1, 0, errno.ENOSPC), (1, 1, None)):
            with self.subTest(blocks=blocks, inodes=inodes), ExitStack() as stack:
                stack.enter_context(patch.object(adapter, "check_parent_isolation"))
                stack.enter_context(patch.object(adapter, "check_environment"))
                stack.enter_context(patch.object(adapter, "check_recipe"))
                stack.enter_context(patch.object(film, "make_simulator", return_value=(None, {})))
                stack.enter_context(patch.object(film, "render", return_value=np.zeros((17, 19, 3))))
                stack.enter_context(patch.object(film, "pixel_digest", side_effect=[
                    reference["input_pixels_sha256"], reference["film_pixels_sha256"],
                    reference["input_pixels_sha256"],
                ]))
                stack.enter_context(patch.object(io, "save_image_oiio", side_effect=OSError("Opaque codec error")))
                stack.enter_context(patch.object(adapter.os, "statvfs", return_value=SimpleNamespace(f_bavail=blocks, f_favail=inodes)))
                signal_policy = stack.enter_context(patch.object(adapter.signal, "signal"))
                with self.assertRaises(OSError) as failure:
                    adapter.produce(value)
                self.assertEqual(failure.exception.errno, expected_errno)
                signal_policy.assert_called_once_with(adapter.signal.SIGXFSZ, adapter.signal.SIG_DFL)

    def test_parent_capability_probes_fail_closed_and_close_acquired_fds(self):
        class LibC:
            acquired = -1
            attached = -1

            def syscall(self, number, *args):
                if number.value == 434:
                    return 101
                self.number = number.value
                adapter.ctypes.set_errno(errno.EPERM)
                return self.acquired

            def ptrace(self, operation, pid, address, data):
                self.operation = operation.value
                adapter.ctypes.set_errno(errno.EPERM)
                return self.attached

        libc = LibC()
        with ExitStack() as stack:
            stack.enter_context(patch.object(adapter.os, "getppid", return_value=1))
            stack.enter_context(patch.object(adapter.platform, "machine", return_value="x86_64"))
            opened = stack.enter_context(patch.object(adapter.os, "open", side_effect=PermissionError(errno.EACCES, "denied")))
            closed = stack.enter_context(patch.object(adapter.os, "close"))
            stack.enter_context(patch.object(adapter.ctypes, "CDLL", return_value=libc))
            adapter.check_parent_isolation()
            self.assertEqual(libc.number, 438)
            self.assertEqual(libc.operation, 0x4206)
            closed.assert_called_once_with(101)
            closed.reset_mock()
            libc.acquired = 102
            with self.assertRaises(ContractError) as rejected:
                adapter.check_parent_isolation()
            self.assertIsNone(rejected.exception.detail)
            self.assertEqual([call.args[0] for call in closed.call_args_list], [102, 101])
            libc.acquired, libc.attached = -1, 0
            with self.assertRaises(ContractError):
                adapter.check_parent_isolation()
            opened.side_effect = None
            opened.return_value = 103
            with self.assertRaises(ContractError):
                adapter.check_parent_isolation()
            closed.assert_called_with(103)
            with patch.object(adapter.platform, "machine", return_value="unknown"):
                with self.assertRaises(ContractError):
                    adapter.check_parent_isolation()

    def test_structured_failures_keep_identity_and_exit_pairing(self):
        for error, outcome, detail, exit_code in (
            (MemoryError(), "allocation-failed", None, 20),
            (OSError(errno.ENOSPC, "private-path"), "storage-full", None, 21),
            (OSError(errno.EFBIG, "private-path"), "storage-full", "output-limit", 21),
            (ContractError("unsupported-input"), "engine-failed", "unsupported-input", 75),
            (ContractError(None), "engine-failed", None, 75),
            (RuntimeError("private-path"), "engine-failed", None, 75),
        ):
            with self.subTest(outcome=outcome, detail=detail):
                with patch.object(adapter, "read_grant", return_value=grant()), \
                        patch.object(adapter, "produce", side_effect=error), \
                        patch.object(adapter, "write_producer") as writer, \
                        patch.object(adapter.sys, "argv", ["adapter.py"]):
                    self.assertEqual(adapter.main(), exit_code)
                    value = writer.call_args.args[0]
                    self.assertEqual(value["outcome"], outcome)
                    self.assertEqual(value["detail"], detail)
                    self.assertEqual(value["launch_id"], grant()["launch_id"])
                    self.assertNotIn("private-path", json.dumps(value))

    def test_loaded_cache_is_rejected_before_numerical_work(self):
        with tempfile.TemporaryDirectory() as directory:
            cache = Path(directory)
            with patch.object(adapter, "CACHE", cache), \
                    patch.dict(os.environ, NUMBA_CACHE_DIR=str(cache)):
                adapter.check_environment()
                (cache / "unqualified.nbc").write_bytes(b"not reusable")
                with self.assertRaises(ContractError):
                    adapter.check_environment()

    def test_tiff_metadata_rejection_and_post_close_storage(self):
        import OpenImageIO as oiio
        from spektrafilm.utils.io import _load_icc_profile

        self.assertEqual(os.getuid(), 0, "The image check target must exercise root-owned sealed inputs")
        profile = _load_icc_profile("ProPhoto RGB", False)
        pixels = np.arange(17 * 19 * 3, dtype=np.float32).reshape(17, 19, 3) / 100
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.tif"
            spec = oiio.ImageSpec(19, 17, 3, oiio.FLOAT)
            profile_array = np.frombuffer(profile, dtype=np.uint8)
            spec.attribute("ICCProfile", oiio.TypeDesc(f"uint8[{len(profile)}]"), profile_array)
            writer = oiio.ImageOutput.create(str(path))
            self.assertTrue(writer.open(str(path), spec))
            self.assertTrue(writer.write_image(pixels))
            self.assertTrue(writer.close())
            path.chmod(0o444)
            value = grant()
            value["fixture"]["source"] = {"kind": "development-tiff", "bytes": path.stat().st_size,
                                          "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            value["input_icc_sha256"] = hashlib.sha256(profile).hexdigest()
            with patch.object(adapter, "INPUT", path):
                decoded = adapter.tiff_pixels(value)
                np.testing.assert_array_equal(decoded, pixels)
                for field in ("width", "height"):
                    invalid = copy.deepcopy(value)
                    invalid["fixture"][field] += 1
                    with self.assertRaises(ContractError):
                        adapter.tiff_pixels(invalid)
                invalid = copy.deepcopy(value)
                invalid["input_icc_sha256"] = "0" * 64
                with self.assertRaises(ContractError):
                    adapter.tiff_pixels(invalid)
                path.unlink()
                # OIIO's ndarray owns a separate allocation, not the reader.
                np.testing.assert_array_equal(decoded, pixels)

    def test_shared_local_plan_vectors(self):
        vectors = json.loads(Path(__file__).with_name("local-plan-vectors.json").read_text())
        self.assertEqual(len(vectors["cases"]), 37)
        for case in vectors["cases"]:
            with self.subTest(case=case["id"]):
                def calculate():
                    if case["operation"] == "gamut":
                        return plan_gamut_workspace(case["pixel_count"], case["workspace_bytes"])
                    if case["operation"] == "cctf":
                        return plan_cctf_workspace(case["pixel_count"], case["workspace_bytes"])
                    shape = (case["height"], case["width"], 3)
                    image = np.broadcast_to(np.zeros((1, 1, 3)), shape)
                    return plan_jpeg_workspace(image, case["workspace_bytes"])
                if "error" in case:
                    with self.assertRaises((ValueError, OverflowError)):
                        calculate()
                else:
                    self.assertEqual(asdict(calculate()), case["expected"])

    def test_grant_plan_mismatch_rejected_before_frame_allocation(self):
        value = grant()
        with patch.object(np, "empty", side_effect=AssertionError("frame allocation")):
            adapter.check_plans(value)
            for name in ("gamut", "cctf", "jpeg"):
                for field in ("batch_pixels", "scratch_bytes", "destination_bytes", "allowance_bytes"):
                    invalid = copy.deepcopy(value)
                    invalid["plan"][name][field] += 1
                    with self.assertRaises(ContractError):
                        adapter.check_plans(invalid)
            invalid = copy.deepcopy(value)
            invalid["fixture"]["width"] = 9569
            with self.assertRaises(ContractError):
                adapter.check_plans(invalid)

    def test_synthetic_inputs_match_independent_references(self):
        references = json.loads(Path(__file__).with_name("references.json").read_text())["references"]
        self.assertEqual(len(references), 7)
        for reference in references:
            with self.subTest(pattern=reference["pattern"]):
                fixture = {"width": reference["width"], "height": reference["height"],
                           "source": {"pattern": reference["pattern"], "seed": reference["seed"]}}
                pixels = adapter.synthetic_pixels(fixture)
                self.assertEqual(pixels.dtype, np.dtype(np.float32))
                self.assertTrue(pixels.flags.c_contiguous)
                self.assertEqual(hashlib.sha256(memoryview(pixels).cast("B")).hexdigest(),
                                 reference["input_pixel_sha256"])

    def test_generator_tail_and_single_pixel(self):
        fixture = {"width": 131, "height": 127, "source": {"pattern": "gradient", "seed": 0}}
        original = np.arange
        previous = None

        def one_temporary_block(*args, **kwargs):
            nonlocal previous
            if previous is not None:
                self.assertIsNone(previous(), "The preceding float64 block is still retained")
            result = original(*args, **kwargs)
            self.assertLessEqual(result.size, 16384)
            previous = weakref.ref(result)
            return result

        with patch.object(np, "arange", side_effect=one_temporary_block):
            pixels = adapter.synthetic_pixels(fixture)
        self.assertIsNone(previous())
        count = 131 * 127
        expected = (np.arange(count, dtype=np.float64) * 4 / (count - 1)).astype(np.float32)
        np.testing.assert_array_equal(pixels.reshape(-1, 3), np.repeat(expected[:, None], 3, axis=1))
        fixture.update(width=1, height=1)
        np.testing.assert_array_equal(adapter.synthetic_pixels(fixture), np.zeros((1, 1, 3), dtype=np.float32))
        fixture["source"]["seed"] = 1
        with self.assertRaises(ContractError):
            adapter.synthetic_pixels(fixture)


if __name__ == "__main__":
    unittest.main()
