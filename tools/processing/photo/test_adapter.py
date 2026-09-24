"""Focused adapter tests: the engine must refuse a wrong or missing
handoff profile instead of letting darktable silently export sRGB.

No darktable is required: the profile check happens before any engine
run, so the refusal exit code is observable on any host. The positive
path (a valid profile proceeding into the engine run) is asserted only
as "not the refusal code"; a real development run must be exercised in
the pinned image (see the Dockerfile and the launcher executor).
"""

import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import adapter  # noqa: E402

# A 512-byte stand-in with the correct header shape but the wrong bytes:
# the real asset is 1276 bytes, so any content mismatch must refuse.
WRONG_PROFILE = struct.pack("<4sIHH", b"abcd", 0, 0x0210, 0) + b"\x00" * 500


class ProfileRefusalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.work = Path(self.tmp.name) / "work"
        (self.work / "config/color/out").mkdir(parents=True)
        self.asset = Path(self.tmp.name) / "asset.icc"

    def tearDown(self):
        self.tmp.cleanup()

    def invoke(self):
        argv = [
            "adapter.py", "--input", str(self.work / "input.raw"),
            "--exposure-milli-ev", "250", "--work", str(self.work),
            "--output", str(self.work / "output/development.tif"),
            "--icc-asset", str(self.asset),
        ]
        original = sys.argv
        sys.argv = argv
        try:
            return adapter.main()
        finally:
            sys.argv = original

    def test_missing_configdir_profile_refuses(self):
        self.asset.write_bytes(WRONG_PROFILE)
        self.assertEqual(self.invoke(), adapter.EXPECTED_REFUSED)
        self.assertFalse((self.work / "reference.db").exists())

    def test_wrong_configdir_profile_refuses(self):
        (self.work / adapter.PROFILE_RELATIVE).write_bytes(WRONG_PROFILE)
        self.asset.write_bytes(WRONG_PROFILE)
        self.assertEqual(self.invoke(), adapter.EXPECTED_REFUSED)

    def test_wrong_asset_refuses(self):
        (self.work / adapter.PROFILE_RELATIVE).write_bytes(WRONG_PROFILE)
        self.asset.write_bytes(WRONG_PROFILE + b"\x01")
        self.assertEqual(self.invoke(), adapter.EXPECTED_REFUSED)

    def test_missing_asset_refuses(self):
        (self.work / adapter.PROFILE_RELATIVE).write_bytes(WRONG_PROFILE)
        self.assertEqual(self.invoke(), adapter.EXPECTED_REFUSED)

    def test_matching_profiles_start_the_engine_run(self):
        # With a matching profile the adapter proceeds into the engine run;
        # without darktable installed this fails as an engine failure, never
        # as the profile refusal.
        profile = Path(__file__).with_name("profile-fixtures")
        for target in ((self.work / adapter.PROFILE_RELATIVE), self.asset):
            target.write_bytes((profile / "linear-prophoto.icc").read_bytes())
        code = self.invoke()
        self.assertNotEqual(code, adapter.EXPECTED_REFUSED)
        self.assertEqual(code, 1)

    def test_exposure_params_encoding(self):
        params = adapter.exposure_params(1.0)
        self.assertEqual(len(params), struct.calcsize("<iffffii"))
        self.assertEqual(struct.unpack("<iffffii", params), (0, 0.0, 1.0, 50.0, -4.0, 0, 0))


if __name__ == "__main__":
    unittest.main()
