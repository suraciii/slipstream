"""Focused byte inspection and HTTP failure-boundary tests for acceptance.py.

Local HTTP fault fixtures exercise required-route failures, redirect refusal,
bounded reads and captured identity checks. Synthetic byte fixtures prove TIFF
and PNG inspectors; they do not qualify any engine or RAW camera. Run after
integration with:

    python3 -m unittest discover -s tools/processing -p 'test_acceptance.py' -v
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import re
import struct
import tempfile
import threading
import unittest
import zlib
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import acceptance

REPO_ROOT = Path(__file__).resolve().parents[2]
PROFILE_ASSET = REPO_ROOT / "crates" / "slipstream-core" / "assets" / "prophoto-linear-g10.icc"

TOKEN = "acceptance-bearer-token-1"
PHOTO_ID = "01234567-1234-5678-9abc-0123456789ef"
FIXTURE_NAME = "DSC00001.ARW"
FIXTURE_BYTES = b"synthetic-raw-fixture-bytes"
BUNDLE_ID = "b" * 64
SOURCE_REVISION = "src-1"
EXPORT_ID = "acceptance-export-1"


def build_development_tiff(
    width: int,
    height: int,
    profile_bytes: bytes,
    *,
    bits=(32, 32, 32),
    sample_format=(3, 3, 3),
    compression: int = 8,
    photometric: int = 2,
    samples: int = 3,
    deflate: bool = True,
) -> bytes:
    """Little-endian float32 RGB TIFF carrying one Deflate strip per row."""
    short_tags = {258: bits, 339: sample_format}
    tags = sorted([256, 257, 258, 259, 262, 273, 277, 278, 279, 339, 34675])
    strip_data = []
    row_samples = width * samples
    for _ in range(height):
        row = struct.pack("<" + "f" * row_samples, *([0.18] * row_samples))
        strip_data.append(zlib.compress(row) if compression == 8 and deflate else row)

    ifd_offset = 8
    ifd_end = ifd_offset + 2 + 12 * len(tags) + 4
    cursor = ifd_end
    offsets = {}
    for tag, values in short_tags.items():
        offsets[tag] = cursor
        cursor += 2 * len(values)
    offsets[34675] = cursor
    cursor += len(profile_bytes)
    strip_offsets_offset = cursor
    cursor += 4 * height
    strip_counts_offset = cursor
    cursor += 4 * height
    strip_offsets = []
    for strip in strip_data:
        strip_offsets.append(cursor)
        cursor += len(strip)

    inline_tags = {
        256: width,
        257: height,
        259: compression,
        262: photometric,
        273: (strip_offsets_offset, height),
        277: samples,
        278: 1,
        279: (strip_counts_offset, height),
    }
    blob = bytearray(b"II" + struct.pack("<H", 42) + struct.pack("<I", ifd_offset))
    blob += struct.pack("<H", len(tags))
    extra = bytearray()
    for tag in tags:
        if tag in (273, 279):
            value_offset, count = inline_tags[tag]
            blob += struct.pack("<HHII", tag, 4, count, value_offset)
        elif tag in inline_tags:
            blob += struct.pack("<HHII", tag, 4, 1, inline_tags[tag])
        elif tag in short_tags:
            values = short_tags[tag]
            blob += struct.pack("<HHII", tag, 3, len(values), offsets[tag])
        elif tag == 34675:
            blob += struct.pack("<HHII", tag, 7, len(profile_bytes), offsets[tag])
        else:
            raise AssertionError(f"unexpected TIFF tag {tag}")
    blob += struct.pack("<I", 0)
    for tag in sorted(short_tags):
        extra += struct.pack("<" + "H" * len(short_tags[tag]), *short_tags[tag])
    extra += profile_bytes
    extra += struct.pack("<" + "I" * height, *strip_offsets)
    extra += struct.pack("<" + "I" * height, *(len(strip) for strip in strip_data))
    extra += b"".join(strip_data)
    return bytes(blob + extra)


def iso_at_now_plus(seconds: float) -> str:
    return (datetime.now(timezone.utc) + timedelta(seconds=seconds)).isoformat()


class HelperTests(unittest.TestCase):
    def test_request_identity_rules(self):
        self.assertTrue(acceptance.valid_request_identity("acceptance-save-a1b2c3d4e5f6"))
        self.assertTrue(acceptance.valid_request_identity("a" * 128))
        self.assertFalse(acceptance.valid_request_identity("a" * 129))
        self.assertFalse(acceptance.valid_request_identity("bad identity"))
        self.assertFalse(acceptance.valid_request_identity(""))
        identity = acceptance.new_request_identity("save")
        self.assertTrue(acceptance.valid_request_identity(identity))
        self.assertIn("-save-", identity)

    def test_timestamp_parsing(self):
        self.assertIsNotNone(acceptance.parse_timestamp("2026-01-01T00:00:00Z"))
        self.assertIsNotNone(acceptance.parse_timestamp("2026-01-01T00:00:00+00:00"))
        self.assertIsNone(acceptance.parse_timestamp("not-a-time"))
        self.assertIsNone(acceptance.parse_timestamp(None))
        self.assertIsNone(acceptance.parse_timestamp(""))

    def test_development_tiff_validation(self):
        profile = PROFILE_ASSET.read_bytes()
        data = build_development_tiff(4, 3, profile)
        facts, problems = acceptance.validate_development_tiff(data, 4, 3)
        self.assertEqual(problems, [])
        self.assertEqual(facts["width"], 4)
        self.assertEqual(facts["bitsPerSample"], [32, 32, 32])
        self.assertEqual(facts["sampleFormat"], [3, 3, 3])
        self.assertTrue(facts["profileAccepted"])
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, profile, bits=(16, 16, 16)), 4, 3
        )
        self.assertIn("tiff-bits-per-sample-not-float32-rgb", problems)
        _, problems = acceptance.validate_development_tiff(data, 7, 3)
        self.assertIn("tiff-width-mismatch", problems)
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, profile, sample_format=(1, 1, 1)), 4, 3
        )
        self.assertIn("tiff-sample-format-not-ieee-float", problems)
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, b"\x00" * 128), 4, 3
        )
        self.assertIn("tiff-embedded-profile-digest-not-pinned", problems)
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, profile, compression=1), 4, 3
        )
        self.assertIn("tiff-compression-not-deflate", problems)
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, profile, photometric=1), 4, 3
        )
        self.assertIn("tiff-photometric-not-rgb", problems)
        _, problems = acceptance.validate_development_tiff(
            build_development_tiff(4, 3, profile, samples=1), 4, 3
        )
        self.assertIn("tiff-samples-per-pixel-not-3", problems)
        # The qualified writer's strip padding stays the qualified shape; a
        # larger unaccounted tail is refused.
        padded = build_development_tiff(4, 3, profile) + b"\x00"
        _, problems = acceptance.validate_development_tiff(padded, 4, 3)
        self.assertEqual(problems, [])
        over = (
            build_development_tiff(4, 3, profile)
            + b"\x00" * (acceptance.STRIP_PADDING_MAXIMUM + 1)
        )
        _, problems = acceptance.validate_development_tiff(over, 4, 3)
        self.assertIn("tiff-unaccounted-trailing-payload", problems)

    def test_development_tiff_rejects_invalid_deflate_stream(self):
        profile = PROFILE_ASSET.read_bytes()
        valid = build_development_tiff(4, 3, profile)
        _, problems = acceptance.validate_development_tiff(valid, 4, 3)
        self.assertEqual(problems, [])

        malformed = build_development_tiff(4, 3, profile, deflate=False)
        _, problems = acceptance.validate_development_tiff(malformed, 4, 3)
        self.assertIn("tiff-deflate-strip-invalid", problems)

    def test_download_limit_clamps_declared_size(self):
        limit, problems = acceptance.artifact_download_limit(100)
        self.assertEqual(limit, 100 + acceptance.DOWNLOAD_SLACK_BYTES)
        self.assertEqual(problems, [])
        limit, problems = acceptance.artifact_download_limit(10**30)
        self.assertEqual(limit, acceptance.MAX_DOWNLOAD_BYTES)
        self.assertEqual(problems, ["declared-byteLength-exceeds-download-limit"])
        limit, problems = acceptance.artifact_download_limit(0)
        self.assertEqual(problems, ["declared-byteLength-not-positive"])
        limit, problems = acceptance.artifact_download_limit("100")
        self.assertEqual(problems, ["declared-byteLength-not-integer"])

    def test_download_bound_option_is_bounded_by_the_hard_maximum(self):
        self.assertEqual(acceptance.download_bound("4294967296"), acceptance.MAXIMUM_DOWNLOAD_BYTES)
        for value in ("0", "-1", "abc", str(acceptance.MAXIMUM_DOWNLOAD_BYTES + 1)):
            with self.assertRaises(argparse.ArgumentTypeError):
                acceptance.download_bound(value)
        parsed = acceptance.parse_args(
            [
                "--base-url", "https://acceptance.example.com",
                "--token-file", "/tmp/token",
                "--fixture", "/tmp/fixture.raw",
                "--output-dir", "/tmp/downloads",
                "--max-download-bytes", "4294967296",
            ]
        )
        self.assertEqual(parsed.max_download_bytes, acceptance.MAXIMUM_DOWNLOAD_BYTES)

    def test_invariance_and_sidecars(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "IMG_0001.dng"
            fixture.write_bytes(b"original-bytes")
            sidecar = root / "IMG_0001.dng.xmp"
            sidecar.write_bytes(b"<xmp/>")
            before = {
                str(fixture): acceptance.snapshot_original(fixture),
                str(sidecar): acceptance.snapshot_original(sidecar),
            }
            self.assertEqual(acceptance.invariance_changes(before, dict(before)), [])
            fixture.write_bytes(b"mutated-bytes")
            after = {
                str(fixture): acceptance.snapshot_original(fixture),
                str(sidecar): acceptance.snapshot_original(sidecar),
            }
            changes = acceptance.invariance_changes(before, after)
            self.assertIn(f"sha256-changed:{fixture}", changes)
            self.assertIn(f"byteLength-changed:{fixture}", changes)
            os.utime(fixture, ns=(1_000_000_000, 1_000_000_000))
            after = {
                str(fixture): acceptance.snapshot_original(fixture),
                str(sidecar): acceptance.snapshot_original(sidecar),
            }
            self.assertIn(f"mtimeNs-changed:{fixture}", acceptance.invariance_changes(before, after))
            self.assertEqual(
                acceptance.invariance_changes(before, {str(sidecar): before[str(sidecar)]}),
                [f"file-disappeared:{fixture}"],
            )
        names = [path.name for path in acceptance.external_xmp_sidecars(Path("/x/IMG_0001.dng"))]
        self.assertEqual(names, ["IMG_0001.dng.xmp", "IMG_0001.xmp"])

    def test_base_url_normalization(self):
        self.assertEqual(acceptance.normalize_base_url("https://photos.example.com"), "https://photos.example.com")
        self.assertEqual(
            acceptance.normalize_base_url("http://127.0.0.1:8080/"), "http://127.0.0.1:8080"
        )
        self.assertEqual(acceptance.normalize_base_url("http://photos.example.com"), "http://photos.example.com")
        with self.assertRaises(acceptance.InvocationRefused):
            acceptance.normalize_base_url("https://user:pass@photos.example.com")
        with self.assertRaises(acceptance.InvocationRefused):
            acceptance.normalize_base_url("https://photos.example.com/api?x=1")

    def test_token_and_output_dir_refusals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            token = root / "token"
            token.write_text("secret\n")
            os.chmod(token, 0o600)
            self.assertEqual(acceptance.read_token_file(token), "secret")
            os.chmod(token, 0o620)
            with self.assertRaises(acceptance.InvocationRefused):
                acceptance.read_token_file(token)
            os.chmod(token, 0o600)
            token.write_text("")
            with self.assertRaises(acceptance.InvocationRefused):
                acceptance.read_token_file(token)
            fixture = root / "fixture.dng"
            fixture.write_bytes(b"data")
            with self.assertRaises(acceptance.InvocationRefused):
                acceptance.prepare_output_dir(str(root / "fixture.dng"), fixture)
            self.assertFalse((root / "fresh").exists())
            output = acceptance.prepare_output_dir(str(root / "fresh"), fixture)
            self.assertTrue(output.is_dir())
            multibyte = root / "token-multibyte"
            multibyte.write_bytes(("é" * 4096).encode("utf-8"))
            os.chmod(multibyte, 0o600)
            with self.assertRaises(acceptance.InvocationRefused) as caught:
                acceptance.read_token_file(multibyte)
            self.assertEqual(caught.exception.reason, "token-file-too-large")
            invalid_utf8 = root / "token-binary"
            invalid_utf8.write_bytes(b"\xff\xfe\xfa")
            os.chmod(invalid_utf8, 0o600)
            with self.assertRaises(acceptance.InvocationRefused) as caught:
                acceptance.read_token_file(invalid_utf8)
            self.assertEqual(caught.exception.reason, "token-file-invalid")

def png_bytes(width=3, height=2):
    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff)
    rows = b"".join(b"\x00" + b"\x40\x50\x60" * width for _ in range(height))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


class HttpFixture:
    """Fault fixture for transport and provenance boundaries, never qualification."""
    def __init__(self, status=200, payload=None, body=None, headers=None):
        self.status, self.payload, self.body = status, payload, body
        self.headers = headers or {}
        self.requests = []

    def __enter__(self):
        fixture = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                fixture.requests.append(self.path)
                data = fixture.body if fixture.body is not None else json.dumps(fixture.payload).encode()
                self.send_response(fixture.status)
                self.send_header("Content-Length", str(len(data)))
                for key, value in fixture.headers.items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(data)
            def log_message(self, *args):
                pass
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base_url = f"http://127.0.0.1:{self.server.server_port}"
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class HttpBoundaryTests(unittest.TestCase):
    def test_required_missing_route_fails_instead_of_qualifying(self):
        with HttpFixture(status=404, body=b"Not Found") as fixture:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                raw = root / FIXTURE_NAME
                raw.write_bytes(FIXTURE_BYTES)
                output = root / "private"
                output.mkdir(mode=0o700)
                runner = acceptance.Runner(base_url=fixture.base_url, token=TOKEN, fixture=raw, output_dir=output)
                report = runner.run()
        self.assertEqual(report["status"], "failed")
        discovery = report["steps"][0]
        self.assertEqual(discovery["status"], "fail")
        self.assertEqual(discovery["reason"], "route-not-deployed")
        self.assertEqual(report["writtenFiles"], [])
        self.assertEqual(report["steps"][-1]["detail"]["unchanged"], True)

    def test_redirect_does_not_forward_bearer(self):
        with HttpFixture(body=b"secret destination") as destination:
            with HttpFixture(status=302, body=b"", headers={"Location": destination.base_url + "/token"}) as origin:
                client = acceptance.Client(origin.base_url, TOKEN)
                response = client.request("GET", "/redirect")
        self.assertEqual(response.status, 302)
        self.assertEqual(destination.requests, [])

    def test_response_bound_refuses_oversized_bytes(self):
        with HttpFixture(body=b"a" * 101) as fixture:
            with self.assertRaises(acceptance.TransportFailure) as caught:
                acceptance.Client(fixture.base_url, TOKEN).request("GET", "/bytes", max_bytes=100)
        self.assertEqual(caught.exception.reason, "response-too-large")

    def test_foreign_captured_recipe_never_reaches_artifact_download(self):
        expected = {"photoId": PHOTO_ID, "stepId": "selected", "module": "darktable",
            "recipeRevision": "r1", "sourceRevision": SOURCE_REVISION,
            "parameters": {"schemaVersion": "darktable-params-1", "tree": acceptance.qualified_darktable_tree(1)},
            "input": {"kind": "original", "photoId": PHOTO_ID, "sourceRevision": SOURCE_REVISION},
            "requestId": EXPORT_ID, "adapterSchemaVersion": "darktable-adapter-1", "bundleId": BUNDLE_ID}
        foreign = dict(expected, state="succeeded", artifactId="artifact", recipeRevision="r2")
        with HttpFixture(payload=foreign) as fixture:
            runner = acceptance.Runner(base_url=fixture.base_url, token=TOKEN, fixture=Path("fixture"), output_dir=Path("output"))
            runner.photo_id, runner.export_request, runner.export_identity = PHOTO_ID, {"requestId": EXPORT_ID}, expected
            with self.assertRaises(acceptance.AcceptanceFailure) as caught:
                runner._step_export_settlement()
        self.assertIn("recipeRevision-mismatch", caught.exception.detail["problems"])
        self.assertEqual(len(fixture.requests), 1)


class SelectedStepTests(unittest.TestCase):
    def test_opaque_source_preserved_and_stale_binding_refused(self):
        source = "opaque\x00" + "界" * 1000
        recipe = {"photoId": PHOTO_ID, "revision": "r1", "sourceRevision": source,
            "currentStepId": "s1", "steps": [{"stepId": "s1", "module": "darktable"}]}
        payload = {"photoId": PHOTO_ID, "sourceRevision": source, "recipe": recipe}
        facts, problems = acceptance.validate_recipe_read(payload, PHOTO_ID)
        self.assertEqual(problems, [])
        self.assertEqual(facts["sourceRevision"], source)
        _, problems = acceptance.validate_recipe_read(dict(payload, sourceRevision="new"), PHOTO_ID)
        self.assertIn("recipe-requires-explicit-rebind", problems)

    def test_updating_selected_step_preserves_unrelated_retained_tree(self):
        runner = acceptance.Runner(base_url="http://127.0.0.1", token=TOKEN, fixture=Path("fixture"), output_dir=Path("output"))
        runner.photo_id, runner.source_revision, runner.step_id = PHOTO_ID, SOURCE_REVISION, "selected"
        retained = {"stepId": "unsupported", "module": "spektrafilm", "input": {"kind": "artifact", "artifactId": "a1"},
                    "parameters": {"schemaVersion": "future", "tree": {"camera": {"unqualified": 19}}}}
        runner.observed_recipe = {"currentStepId": "selected", "steps": [retained]}
        changed = runner._recipe_intent(1.0)
        self.assertEqual(changed["steps"][0], retained)
        changed["steps"][0]["parameters"]["tree"]["camera"]["unqualified"] = 20
        self.assertEqual(retained["parameters"]["tree"]["camera"]["unqualified"], 19)

    def test_png_rejects_geometry_corruption_and_truncation(self):
        facts, problems = acceptance.validate_png(png_bytes(7, 5), 1224)
        self.assertEqual(problems, [])
        self.assertEqual((facts["width"], facts["height"]), (7, 5))
        _, problems = acceptance.validate_png(png_bytes(7, 5), 6)
        self.assertIn("png-geometry-exceeds-bound", problems)
        data = bytearray(png_bytes())
        data[29] ^= 1
        self.assertIn("png-crc-invalid", acceptance.validate_png(bytes(data), 1224)[1])
        self.assertIn("png-chunk-truncated", acceptance.validate_png(png_bytes()[:-2], 1224)[1])

    def test_private_output_rejects_shared_directory_and_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / "fixture.ARW"
            raw.write_bytes(b"raw")
            shared = root / "shared"
            shared.mkdir(mode=0o755)
            # mkdir's mode is filtered by the operator's umask; set the unsafe
            # permissions explicitly so a private umask cannot hide this case.
            shared.chmod(0o755)
            with self.assertRaises(acceptance.InvocationRefused) as caught:
                acceptance.prepare_output_dir(str(shared), raw)
            self.assertEqual(caught.exception.reason, "output-dir-not-private")
            link = root / "link"
            link.symlink_to(shared)
            with self.assertRaises(acceptance.InvocationRefused) as caught:
                acceptance.prepare_output_dir(str(link), raw)
            self.assertEqual(caught.exception.reason, "output-dir-symlink")

    def test_nonfinite_timeout_refused_before_network(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = acceptance.main(["--base-url", "https://unused.example", "--token-file", "/missing",
                "--fixture", "/missing", "--output-dir", "/missing", "--settlement-timeout", "nan",
                "--i-acknowledge-this-is-an-acceptance-instance"])
        self.assertEqual(code, 2)
        self.assertEqual(json.loads(output.getvalue())["reason"], "timeout-outside-finite-bound")


if __name__ == "__main__":
    unittest.main()
