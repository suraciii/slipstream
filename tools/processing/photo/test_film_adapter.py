"""Focused film-adapter tests: the pinned handoff and output identities.

No numerical runtime is required. Everything observable on a host is the
fail-closed path: without the sealed development source, outside the pinned
deterministic environment, or with a handoff/output spec that misses the
pinned ICC identity or bounded geometry, the adapter refuses before any
engine work. The positive render path requires the pinned image (see the
Dockerfile and the launcher executor) and is never asserted here.
"""

import os
import sys
import tempfile
import unittest
import unittest.mock
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import film_adapter  # noqa: E402


class FakeSpec:
    """The OIIO spec surface the adapter reads, as plain attributes."""

    def __init__(self, width=1224, height=816, nchannels=3, depth=1,
                 channelformats=(), x=0, y=0, z=0, orientation=1, profile=b"profile"):
        self.width = width
        self.height = height
        self.nchannels = nchannels
        self.depth = depth
        self.channelformats = list(channelformats)
        self.x = x
        self.y = y
        self.z = z
        self.full_width = width
        self.full_height = height
        self.full_depth = 1
        self.format = "float"
        self._profile = profile
        self._orientation = orientation

    def get_int_attribute(self, name, default):
        assert name == "Orientation"
        return self._orientation

    def getattribute(self, name):
        assert name == "ICCProfile"
        return self._profile


class GeometryBoundsTest(unittest.TestCase):
    def test_bounded_frames_are_admitted(self):
        self.assertTrue(film_adapter.geometry_bounds(1, 1))
        self.assertTrue(film_adapter.geometry_bounds(9568, 1))
        self.assertTrue(film_adapter.geometry_bounds(1224, 816))

    def test_unbounded_or_degenerate_frames_are_refused(self):
        for width, height in [
            (0, 1), (1, 0), (-1, 100), (9569, 1), (1, 9569),
            (30000, 30000),
        ]:
            self.assertFalse(
                film_adapter.geometry_bounds(width, height),
                f"{width}x{height} must be refused",
            )


class ProfileIdentityTest(unittest.TestCase):
    """Exact embedded profile bytes; a lookalike profile is not evidence."""

    def test_exact_bytes_are_the_only_evidence(self):
        digest = film_adapter.sha256_bytes(b"profile")
        self.assertFalse(film_adapter.profile_is_pinned(b"lookalike", digest))
        self.assertTrue(film_adapter.profile_is_pinned(b"profile", digest))

    def test_missing_or_oversized_profiles_are_refused(self):
        digest = film_adapter.sha256_bytes(b"profile")
        self.assertFalse(film_adapter.profile_is_pinned(None, digest))
        self.assertFalse(film_adapter.profile_is_pinned(b"", digest))
        self.assertFalse(film_adapter.profile_is_pinned(b"x" * 16385, digest))


class InputSpecTest(unittest.TestCase):
    """The handoff contract: float32 RGB, full frame, pinned ProPhoto ICC."""

    def test_a_pinned_handoff_spec_passes(self):
        with unittest.mock.patch.object(
            film_adapter, "INPUT_ICC_SHA256", film_adapter.sha256_bytes(b"pinned")
        ):
            spec = FakeSpec(profile=b"pinned")
            self.assertTrue(film_adapter.input_spec_is_pinned(spec, "float", b"pinned"))

    def test_handoff_spec_deviations_are_refused(self):
        profile = b"pinned"
        with unittest.mock.patch.object(
            film_adapter, "INPUT_ICC_SHA256", film_adapter.sha256_bytes(profile)
        ):
            unbounded = FakeSpec(width=9569, profile=profile)
            uint8 = FakeSpec(profile=profile)
            uint8.format = "uint8"
            cropped = FakeSpec(profile=profile)
            cropped.full_width = 100
            deviations = {
                "wrong-channels": FakeSpec(nchannels=4, profile=profile),
                "per-channel-formats": FakeSpec(
                    channelformats=["float"] * 3, profile=profile
                ),
                "wrong-sample-format": uint8,
                "volume-data": FakeSpec(depth=2, profile=profile),
                "offset-frame": FakeSpec(x=4, profile=profile),
                "cropped-full-frame": cropped,
                "rotated": FakeSpec(orientation=6, profile=profile),
                "foreign-profile": FakeSpec(profile=b"other"),
                "missing-profile": FakeSpec(profile=None),
                "unbounded": unbounded,
            }
            for name, spec in deviations.items():
                self.assertFalse(
                    film_adapter.input_spec_is_pinned(
                        spec, str(spec.format), spec.getattribute("ICCProfile")
                    ),
                    f"{name} must be refused",
                )


class FinishedSpecTest(unittest.TestCase):
    """The output contract: sRGB ICC, jpeg container, exact input geometry."""

    def test_a_pinned_finished_spec_passes(self):
        profile = b"srgb"
        with unittest.mock.patch.object(
            film_adapter, "OUTPUT_ICC_SHA256", film_adapter.sha256_bytes(profile)
        ):
            self.assertTrue(
                film_adapter.finished_spec_is_pinned(
                    "jpeg", FakeSpec(width=64, height=48), profile, 64, 48
                )
            )

    def test_finished_spec_deviations_are_refused(self):
        profile = b"srgb"
        with unittest.mock.patch.object(
            film_adapter, "OUTPUT_ICC_SHA256", film_adapter.sha256_bytes(profile)
        ):
            deviations = [
                ("not-jpeg", ("tiff", FakeSpec(width=64, height=48), profile, 64, 48)),
                ("geometry", ("jpeg", FakeSpec(width=64, height=48), profile, 64, 50)),
                ("channels", ("jpeg", FakeSpec(width=64, height=48, nchannels=1),
                              profile, 64, 48)),
                ("foreign-profile", ("jpeg", FakeSpec(width=64, height=48),
                                     b"other", 64, 48)),
                ("missing-profile", ("jpeg",
                                     FakeSpec(width=64, height=48, profile=None),
                                     None, 64, 48)),
            ]
            for name, arguments in deviations:
                self.assertFalse(
                    film_adapter.finished_spec_is_pinned(*arguments),
                    f"{name} must be refused",
                )


class EnvironmentTest(unittest.TestCase):
    ENV_KEYS = (
        "NUMBA_CACHE_DIR", "NUMBA_NUM_THREADS", "OMP_NUM_THREADS",
        "OPENBLAS_NUM_THREADS", "NUMEXPR_NUM_THREADS",
    )

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.cache = Path(self.tmp.name) / "numba"
        self.cache.mkdir()
        self.original = {key: os.environ.get(key) for key in self.ENV_KEYS}

    def tearDown(self):
        for key, value in self.original.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        self.tmp.cleanup()

    def with_pinned_environment(self):
        pinned = {
            "NUMBA_CACHE_DIR": str(self.cache),
            "NUMBA_NUM_THREADS": "1",
            "OMP_NUM_THREADS": "4",
            "OPENBLAS_NUM_THREADS": "4",
            "NUMEXPR_NUM_THREADS": "4",
        }
        os.environ.update(pinned)

    def test_the_pinned_environment_and_a_fresh_cache_pass(self):
        self.with_pinned_environment()
        with unittest.mock.patch.object(film_adapter, "NUMBA_CACHE", self.cache):
            self.assertTrue(film_adapter.check_environment())

    def test_missing_variable_or_used_cache_refuses(self):
        self.with_pinned_environment()
        del os.environ["NUMEXPR_NUM_THREADS"]
        with unittest.mock.patch.object(film_adapter, "NUMBA_CACHE", self.cache):
            self.assertFalse(film_adapter.check_environment())
        self.with_pinned_environment()
        (self.cache / "kernel").write_bytes(b"compiled")
        with unittest.mock.patch.object(film_adapter, "NUMBA_CACHE", self.cache):
            self.assertFalse(film_adapter.check_environment())


class MainRefusalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.source = Path(self.tmp.name) / "missing.tif"
        self.output = Path(self.tmp.name) / "finished.jpg"

    def tearDown(self):
        self.tmp.cleanup()

    def invoke(self):
        argv = [
            "film_adapter.py",
            "--input", str(self.source),
            "--output", str(self.output),
        ]
        original = sys.argv
        sys.argv = argv
        try:
            return film_adapter.main()
        finally:
            sys.argv = original

    def test_without_the_sealed_source_the_adapter_refuses(self):
        self.assertEqual(self.invoke(), film_adapter.REFUSED)
        self.assertFalse(self.output.exists())

    def test_an_existing_output_is_never_overwritten(self):
        self.output.write_bytes(b"prior artifact")
        try:
            self.assertEqual(self.invoke(), film_adapter.REFUSED)
            self.assertEqual(self.output.read_bytes(), b"prior artifact")
        finally:
            self.output.unlink()


if __name__ == "__main__":
    unittest.main()
