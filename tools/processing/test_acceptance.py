"""Focused unit tests and stub-deployment dry runs for acceptance.py.

The stub deployment in this module implements the merged wire contract of
`design/photo-development.md` well enough to exercise every runner step without
a real deployment.  Run with:

    python3 -m unittest discover -s tools/processing -p 'test_*.py' -v
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
FIXTURE_NAME = "P1020001.dng"
FIXTURE_BYTES = b"synthetic-raw-fixture-bytes"
BUNDLE_ID = "b" * 64
SOURCE_REVISION = "src-1"
EXPORT_ID = "acceptance-export-1"


def minimal_jpeg(width: int = 3, height: int = 2) -> bytes:
    """A marker-valid grayscale JPEG; the runner's walker parses its SOF."""
    return b"".join(
        [
            b"\xff\xd8",
            b"\xff\xc0" + struct.pack(">H", 11) + bytes([8])
            + struct.pack(">H", height) + struct.pack(">H", width)
            + bytes([1, 1, 0x11, 0]),
            b"\xff\xda" + struct.pack(">H", 8) + bytes([1, 1, 0, 0, 0x3F, 0]),
            b"\x00",
            b"\xff\xd9",
        ]
    )


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


class StubDeployment:
    """In-process stub of the merged Photo development wire contract."""

    def __init__(
        self,
        *,
        capability_state: str = "ready",
        develop_stage: str = "ready",
        fail_export: bool = False,
        export_running_polls: int = 1,
        disabled_routes: tuple = (),
        duplicate_fixture: bool = False,
        preview_202_first: bool = False,
        preview_202_always: bool = False,
        wrong_token: bool = False,
        inspect_photo_id: str | None = None,
        submit_recipe_version: str | None = None,
        artifact_export_id: str = EXPORT_ID,
        preview_content_type: str = "image/jpeg",
        artifact_bytes_override: bytes | None = None,
        preview_body_override: bytes | None = None,
        list_page_maximum: int = 60,
        query_pages: list | None = None,
    ):
        self.capability_payload = {
            "state": capability_state,
            "bundleId": BUNDLE_ID,
            "incarnation": "a" * 32,
            "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
            "profiles": [
                {"profileId": "raw", "whiteBalanceModes": ["as-shot"], "whiteBalanceRanges": None}
            ],
            "stages": {"develop": develop_stage, "film": "unavailable"},
        }
        photos = [
            {
                "id": PHOTO_ID,
                "filename": FIXTURE_NAME,
                "originalKind": "raw",
                "originalAvailable": True,
            }
        ]
        if duplicate_fixture:
            photos.append(dict(photos[0], id="ffffffff-1234-5678-9abc-0123456789ef"))
        self.photos = photos
        self.recipe = None
        self.recipe_counter = 0
        self.source_revision = SOURCE_REVISION
        self.profile_bytes = PROFILE_ASSET.read_bytes()
        self.artifact_bytes = (
            artifact_bytes_override
            if artifact_bytes_override is not None
            else build_development_tiff(4, 3, self.profile_bytes)
        )
        self.preview_bytes = minimal_jpeg()
        self.fail_export = fail_export
        self.export_running_polls = export_running_polls
        self.export_inspections = 0
        self.disabled_routes = set(disabled_routes)
        self.preview_202_first = preview_202_first
        self.preview_202_always = preview_202_always
        self.preview_polls = 0
        self.wrong_token = wrong_token
        self.inspect_photo_id = inspect_photo_id
        self.submit_recipe_version = submit_recipe_version
        self.artifact_export_id = artifact_export_id
        self.preview_content_type = preview_content_type
        self.preview_body_override = preview_body_override
        self.list_page_maximum = list_page_maximum
        self.query_pages = query_pages if query_pages is not None else [self.photos]
        self.requests: list[dict] = []
        self.export_recipe_version: str | None = None
        self.artifact_expiry = iso_at_now_plus(7 * 86400)

    def artifact_metadata(self) -> dict:
        return {
            "exportId": self.artifact_export_id,
            "target": "development-tiff",
            "stage": "develop",
            "contentType": "image/tiff",
            "width": 4,
            "height": 3,
            "profileIdentity": hashlib.sha256(self.profile_bytes).hexdigest(),
            "byteLength": len(self.artifact_bytes),
            "sha256": hashlib.sha256(self.artifact_bytes).hexdigest(),
            "expiresAt": self.artifact_expiry,
        }

    def recipe_read(self) -> dict:
        return {
            "photoId": PHOTO_ID,
            "sourceRevision": self.source_revision,
            "recipe": self.recipe,
            "sourceSupport": "supported",
            "supportReason": None,
            "processingAvailable": True,
            "controls": {
                "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
                "whiteBalanceModes": ["as-shot"],
            },
        }

    def guarded_save(self, body: dict) -> tuple[int, dict]:
        if body.get("expectedSourceRevision") != self.source_revision:
            return 409, {
                "error": {
                    "code": "source_changed",
                    "message": "stub",
                    "details": {
                        "currentSourceRevision": self.source_revision,
                        "currentRecipeVersion": self.recipe["recipeVersion"] if self.recipe else None,
                    },
                }
            }
        expected = body.get("expectedRecipeVersion")
        current = self.recipe["recipeVersion"] if self.recipe else None
        if expected != current:
            return 409, {
                "error": {
                    "code": "recipe_conflict",
                    "message": "stub",
                    "details": {
                        "currentSourceRevision": self.source_revision,
                        "currentRecipeVersion": current,
                    },
                }
            }
        self.recipe_counter += 1
        version = f"rv-{self.recipe_counter}"
        self.recipe = {
            "recipeVersion": version,
            "exposureEv": body["settings"]["exposureEv"],
            "whiteBalance": body["settings"]["whiteBalance"],
        }
        return 200, {"outcome": "saved", "recipeVersion": version, "sourceRevision": self.source_revision}

    def export_inspect(self) -> dict:
        self.export_inspections += 1
        artifact = self.artifact_metadata()
        active = self.export_inspections <= self.export_running_polls
        if self.fail_export and not active:
            return {
                "exportId": EXPORT_ID,
                "photoId": self.inspect_photo_id or PHOTO_ID,
                "state": "failed",
                "target": "development-tiff",
                "recipeVersion": self.export_recipe_version,
                "sourceRevision": self.source_revision,
                "bundleId": BUNDLE_ID,
                "terminalOutcome": "failed",
                "failureReason": "engine-failed",
                "receiptExpiresAt": iso_at_now_plus(7 * 86400),
                "artifact": None,
            }
        if active:
            return {
                "exportId": EXPORT_ID,
                "photoId": self.inspect_photo_id or PHOTO_ID,
                "state": "running",
                "target": "development-tiff",
                "recipeVersion": self.export_recipe_version,
                "sourceRevision": self.source_revision,
                "bundleId": BUNDLE_ID,
                "terminalOutcome": None,
                "failureReason": None,
                "receiptExpiresAt": None,
                "artifact": None,
            }
        return {
            "exportId": EXPORT_ID,
            "photoId": self.inspect_photo_id or PHOTO_ID,
            "state": "succeeded",
            "target": "development-tiff",
            "recipeVersion": self.export_recipe_version,
            "sourceRevision": self.source_revision,
            "bundleId": BUNDLE_ID,
            "terminalOutcome": "succeeded",
            "failureReason": None,
            "receiptExpiresAt": iso_at_now_plus(7 * 86400),
            "artifact": artifact,
        }

    def capabilities_payload(self) -> dict:
        return {
            "serverVersion": "0.0.0-stub",
            "supportedCliContractVersions": [1],
            "limits": {
                "listPageMaximum": self.list_page_maximum,
                "mutationPhotoIdsMaximum": 100,
                "albumReorderMembersMaximum": 100,
                "retainedQueryIdsMaximum": 100,
                "retainedQueryIdleSeconds": 60,
            },
        }

    @staticmethod
    def _invalid_input(argument: str, reason: str) -> tuple[int, bytes, list]:
        return (
            400,
            json.dumps(
                {
                    "error": {
                        "code": "invalid_input",
                        "message": "The request is invalid.",
                        "effect": "none",
                        "details": {"argument": argument, "reason": reason},
                    }
                }
            ).encode(),
            [],
        )

    def photo_query(self, body: object) -> tuple[int, bytes, list]:
        """Enforce what `create_photo_query` enforces on the merged server."""
        if not isinstance(body, dict):
            return self._invalid_input("body", "The Photo query body is invalid.")
        allowed = {
            "source",
            "selection",
            "ratingMinimum",
            "ratingMaximum",
            "kind",
            "available",
            "capturedFrom",
            "capturedBefore",
            "order",
            "limit",
        }
        if set(body) - allowed:
            return self._invalid_input("body", "The Photo query body is invalid.")
        source = body.get("source")
        # The source travels as the externally tagged enum (`tag = "kind"`):
        # an untagged string like the legacy `"source": "all"` must refuse.
        if source is not None and not (
            isinstance(source, dict)
            and isinstance(source.get("kind"), str)
            and source.get("kind") in ("all", "album", "folder")
        ):
            return self._invalid_input("body", "The Photo query body is invalid.")
        limit = body.get("limit", self.list_page_maximum)
        if not isinstance(limit, int) or isinstance(limit, bool) or not (
            1 <= limit <= self.list_page_maximum
        ):
            return self._invalid_input("limit", f"The limit must be from 1 through {self.list_page_maximum}.")
        kind = body.get("kind")
        if kind is not None and kind not in ("raw", "jpeg"):
            return self._invalid_input("kind", "The Original kind filter is invalid.")
        available = body.get("available")
        if available is not None and not isinstance(available, bool):
            return self._invalid_input("available", "The Original availability filter is invalid.")
        page = self.query_pages[0]
        next_cursor = "query-cursor-1" if len(self.query_pages) > 1 else None
        return (
            200,
            json.dumps({"items": page, "total": sum(len(p) for p in self.query_pages), "nextCursor": next_cursor}).encode(),
            [],
        )

    def photo_query_page(self, cursor: str) -> tuple[int, bytes, list]:
        if not cursor.startswith("query-cursor-"):
            return 410, json.dumps({"error": {"code": "cursor_expired", "message": "stub"}}).encode(), []
        index = int(cursor.rsplit("-", 1)[1])
        page = self.query_pages[index]
        next_cursor = f"query-cursor-{index + 1}" if index + 1 < len(self.query_pages) else None
        return (
            200,
            json.dumps({"items": page, "total": sum(len(p) for p in self.query_pages), "nextCursor": next_cursor}).encode(),
            [],
        )

    def handle(self, method: str, path: str, body: bytes, headers) -> tuple[int, bytes, list]:
        # The merged server validates the CLI contract header (and refuses
        # with 426) before anything else on these routes.
        if headers.get("slipstream-cli-contract") != "1":
            return 426, json.dumps({"error": {"code": "incompatible_server", "message": "stub"}}).encode(), []
        if headers.get("Authorization") != f"Bearer {'wrong' if self.wrong_token else TOKEN}":
            return 401, json.dumps({"error": {"code": "unauthorized", "message": "stub"}}).encode(), []
        try:
            parsed_body: object = json.loads(body) if body else None
        except (UnicodeDecodeError, json.JSONDecodeError):
            return self._invalid_input("body", "The request body must be one valid JSON object.")
        self.requests.append({"method": method, "path": path, "body": parsed_body})
        if path == "/api/capabilities" and method == "GET":
            return 200, json.dumps(self.capabilities_payload()).encode(), []
        if path == "/api/processing/capability":
            if "capability" in self.disabled_routes:
                return 404, b"<html>not found</html>", []
            return 200, json.dumps(self.capability_payload).encode(), []
        if path == "/api/photo-queries" and method == "POST":
            return self.photo_query(parsed_body)
        if path.startswith("/api/photo-queries/") and method == "GET":
            return self.photo_query_page(path.rsplit("/", 1)[1])
        if path == f"/api/photos/{PHOTO_ID}/edit-recipe":
            if "edit-recipe" in self.disabled_routes:
                return 404, b"", []
            if method == "GET":
                return 200, json.dumps(self.recipe_read()).encode(), []
            refused = self.validate_save_body(parsed_body)
            if refused is not None:
                return refused
            status, payload = self.guarded_save(parsed_body)
            return status, json.dumps(payload).encode(), []
        if path == f"/api/photos/{PHOTO_ID}/edit-preview/develop" and method == "GET":
            if "edit-preview" in self.disabled_routes:
                return 404, b"", []
            self.preview_polls += 1
            if (self.preview_202_first and self.preview_polls == 1) or self.preview_202_always:
                return 202, json.dumps({"state": "queued", "stage": "develop"}).encode(), []
            return 200, *self.preview_response()
        if path == f"/api/photos/{PHOTO_ID}/exports" and method == "POST":
            if "exports" in self.disabled_routes:
                return 404, b"", []
            return self.submit_export(parsed_body)
        if path == f"/api/exports/{EXPORT_ID}" and method == "GET":
            return 200, json.dumps(self.export_inspect()).encode(), []
        if path == f"/api/exports/{EXPORT_ID}/artifact" and method == "GET":
            return 200, *self.artifact_response()
        return 404, b"", []

    @staticmethod
    def _invalid_settings(argument: str, reason: str) -> tuple[int, bytes, list]:
        return (
            422,
            json.dumps(
                {
                    "error": {
                        "code": "invalid_settings",
                        "message": "Correct the settings against the approved ranges and shape.",
                        "details": {"argument": argument, "reason": reason},
                    }
                }
            ).encode(),
            [],
        )

    def validate_save_body(self, body: object) -> tuple[int, bytes, list] | None:
        """Enforce the `deny_unknown_fields` save shape the server enforces."""
        if not isinstance(body, dict) or set(body) != {
            "requestId",
            "expectedRecipeVersion",
            "expectedSourceRevision",
            "settings",
        }:
            return self._invalid_settings("body", "The body is malformed or contains unknown fields.")
        identity = body["requestId"]
        if not isinstance(identity, str) or not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", identity):
            return self._invalid_settings("requestId", "The request identity is outside the closed shape.")
        if body["expectedRecipeVersion"] is not None and not isinstance(
            body["expectedRecipeVersion"], str
        ):
            return self._invalid_settings("expectedRecipeVersion", "Use a string or null.")
        if not isinstance(body["expectedSourceRevision"], str) or not body["expectedSourceRevision"]:
            return self._invalid_settings("expectedSourceRevision", "Use a nonempty string.")
        settings = body["settings"]
        if not isinstance(settings, dict) or set(settings) != {"exposureEv", "whiteBalance"}:
            return self._invalid_settings("settings", "The settings are malformed.")
        if not isinstance(settings["exposureEv"], (int, float)) or isinstance(
            settings["exposureEv"], bool
        ):
            return self._invalid_settings("exposureEv", "Use a finite number of EV.")
        white_balance = settings["whiteBalance"]
        if not isinstance(white_balance, dict) or white_balance.get("mode") != "as-shot":
            return self._invalid_settings("whiteBalance", "The mode is not admitted.")
        return None

    def submit_export(self, body: object) -> tuple[int, bytes, list]:
        if not isinstance(body, dict) or set(body) != {
            "requestId",
            "expectedRecipeVersion",
            "expectedSourceRevision",
            "target",
        }:
            return self._invalid_settings("body", "The submission carries a value outside the closed wire shape")
        identity = body["requestId"]
        if not isinstance(identity, str) or not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", identity):
            return self._invalid_settings("requestId", "The request identity is outside the closed shape")
        if (
            not isinstance(body["expectedRecipeVersion"], str)
            or not body["expectedRecipeVersion"]
            or not isinstance(body["expectedSourceRevision"], str)
            or not body["expectedSourceRevision"]
            or body["target"] != "development-tiff"
        ):
            return self._invalid_settings("target", "The submission carries a value outside the closed wire shape")
        if body["expectedSourceRevision"] != self.source_revision:
            return 409, json.dumps({"error": {"code": "source_changed", "message": "stub"}}).encode(), []
        current = self.recipe["recipeVersion"] if self.recipe else None
        if body["expectedRecipeVersion"] != current:
            return 409, json.dumps({"error": {"code": "recipe_conflict", "message": "stub"}}).encode(), []
        self.export_recipe_version = (
            self.submit_recipe_version if self.submit_recipe_version is not None else current
        )
        return (
            201,
            json.dumps(
                {
                    "exportId": EXPORT_ID,
                    "state": "queued",
                    "target": "development-tiff",
                    "recipeVersion": self.export_recipe_version,
                    "sourceRevision": self.source_revision,
                    "receiptExpiresAt": None,
                    "artifactExpiresAt": None,
                }
            ).encode(),
            [],
        )

    def preview_response(self) -> tuple[bytes, list]:
        preview_body = self.preview_body_override or self.preview_bytes
        metadata = [
            ("Content-Type", self.preview_content_type),
            ("slipstream-edit-preview-photo-id", PHOTO_ID),
            ("slipstream-edit-preview-stage", "develop"),
            ("slipstream-edit-preview-width", "3"),
            ("slipstream-edit-preview-height", "2"),
            ("slipstream-edit-preview-sha256", hashlib.sha256(preview_body).hexdigest()),
            ("slipstream-edit-preview-source-revision", self.source_revision.encode().hex()),
            (
                "slipstream-edit-preview-recipe-version",
                self.recipe["recipeVersion"] if self.recipe else "",
            ),
            ("slipstream-edit-preview-display-transform", "display-transform-v1"),
            ("slipstream-edit-preview-expires-at", iso_at_now_plus(3600)),
        ]
        return preview_body, metadata

    def artifact_response(self) -> tuple[bytes, list]:
        metadata = self.artifact_metadata()
        headers = [
            ("Content-Type", "image/tiff"),
            ("slipstream-artifact-export-id", metadata["exportId"]),
            ("slipstream-artifact-target", metadata["target"]),
            ("slipstream-artifact-stage", metadata["stage"]),
            ("slipstream-artifact-content-type", metadata["contentType"]),
            ("slipstream-artifact-width", str(metadata["width"])),
            ("slipstream-artifact-height", str(metadata["height"])),
            ("slipstream-artifact-profile-identity", metadata["profileIdentity"]),
            ("slipstream-artifact-byte-length", str(metadata["byteLength"])),
            ("slipstream-artifact-sha256", metadata["sha256"]),
            ("slipstream-artifact-expires-at", metadata["expiresAt"]),
        ]
        return self.artifact_bytes, headers


def make_handler(stub: StubDeployment):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def _dispatch(self, method: str):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            status, payload, metadata = stub.handle(method, self.path, body, self.headers)
            explicit_type = next(
                (value for name, value in metadata if name.lower() == "content-type"), None
            )
            content_type = explicit_type or (
                "application/json" if payload[:1] in (b"{", b"[") or status in (202, 401) or not metadata
                else "application/octet-stream"
            )
            self.send_response(status)
            for name, value in metadata:
                if name.lower() == "content-type":
                    continue
                self.send_header(name, value)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            self._dispatch("GET")

        def do_POST(self):
            self._dispatch("POST")

    return Handler


class RunningStub:
    """Context manager serving one StubDeployment on a loopback port."""

    def __init__(self, stub: StubDeployment):
        self.stub = stub
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(stub))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()

    @property
    def base_url(self) -> str:
        host, port = self.server.server_address[:2]
        return f"http://{host}:{port}"


def run_main(argv: list[str]) -> tuple[int, dict, str]:
    stdout, stderr = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        code = acceptance.main(argv)
    return code, json.loads(stdout.getvalue()), stderr.getvalue()


class AcceptanceTestCase(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.fixture = self.root / "incoming" / FIXTURE_NAME
        self.fixture.parent.mkdir()
        self.fixture.write_bytes(FIXTURE_BYTES)
        self.output_dir = self.root / "downloads"
        self.token_file = self.root / "token"
        self.token_file.write_text(TOKEN + "\n")
        os.chmod(self.token_file, 0o600)

    def invocation(self, stub: RunningStub, extra: list[str] | None = None) -> list[str]:
        return [
            "--base-url", stub.base_url,
            "--token-file", str(self.token_file),
            "--fixture", str(self.fixture),
            "--output-dir", str(self.output_dir),
            "--i-acknowledge-this-is-an-acceptance-instance",
            "--poll-interval", "0.01",
            "--settlement-timeout", "30",
            "--preview-timeout", "30",
        ] + (extra or [])


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

    def test_structured_code_reads_the_merged_error_envelope(self):
        """The merged server nests refusal codes under `error` (`cli_error`)."""
        self.assertEqual(
            acceptance.structured_code(
                {
                    "error": {
                        "code": "processing_unavailable",
                        "message": "The develop stage cannot execute for this Photo right now.",
                        "details": {"reason": "preview-render-admission-unavailable"},
                    }
                }
            ),
            "processing_unavailable",
        )
        self.assertEqual(acceptance.structured_code({"code": "unknown_photo"}), "unknown_photo")
        self.assertIsNone(acceptance.structured_code({"message": "no code here"}))
        self.assertIsNone(acceptance.structured_code(None))

    def test_timestamp_parsing(self):
        self.assertIsNotNone(acceptance.parse_timestamp("2026-01-01T00:00:00Z"))
        self.assertIsNotNone(acceptance.parse_timestamp("2026-01-01T00:00:00+00:00"))
        self.assertIsNone(acceptance.parse_timestamp("not-a-time"))
        self.assertIsNone(acceptance.parse_timestamp(None))
        self.assertIsNone(acceptance.parse_timestamp(""))

    def test_capability_validation(self):
        facts, problems = acceptance.validate_capability(
            {
                "state": "ready",
                "bundleId": "b" * 64,
                "incarnation": "a" * 32,
                "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
                "profiles": [{"profileId": "raw", "whiteBalanceModes": ["as-shot"]}],
                "stages": {"develop": "ready", "film": "unavailable"},
            }
        )
        self.assertEqual(problems, [])
        self.assertEqual(facts["state"], "ready")
        _, problems = acceptance.validate_capability({"state": "turbo"})
        self.assertIn("state-outside-closed-set", problems)
        self.assertIn("stages-must-name-develop-and-film", problems)
        _, problems = acceptance.validate_capability(
            {
                "state": "disabled",
                "bundleId": None,
                "incarnation": None,
                "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
                "profiles": [],
                "stages": {"develop": "unavailable", "film": "unavailable"},
            }
        )
        self.assertEqual(problems, [])
        _, problems = acceptance.validate_capability(
            {
                "state": "ready",
                "bundleId": "bundle",
                "incarnation": "inc-1",
                "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
                "profiles": [
                    {
                        "profileId": "raw",
                        "whiteBalanceModes": ["as-shot", 1],
                        "whiteBalanceRanges": {"temperature-tint": "wide"},
                    }
                ],
                "stages": {"develop": "ready", "film": "unavailable"},
            }
        )
        self.assertIn("bundleId-not-lowercase-hex-64", problems)
        self.assertIn("incarnation-not-lowercase-hex-32", problems)
        self.assertIn("profiles-0-whiteBalanceModes-element-not-string", problems)
        self.assertIn("profiles-0-whiteBalanceRanges-entry-invalid", problems)

    def test_recipe_read_validation(self):
        good = {
            "photoId": PHOTO_ID,
            "sourceRevision": "src-1",
            "recipe": None,
            "sourceSupport": "supported",
            "supportReason": None,
            "processingAvailable": True,
            "controls": {
                "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5},
                "whiteBalanceModes": ["as-shot"],
            },
        }
        facts, problems = acceptance.validate_recipe_read(good, PHOTO_ID)
        self.assertEqual(problems, [])
        self.assertEqual(facts["sourceRevision"], "src-1")
        unavailable = dict(good, sourceSupport="unavailable", supportReason=None)
        _, problems = acceptance.validate_recipe_read(unavailable, PHOTO_ID)
        self.assertIn("sourceRevision-must-be-null-when-unavailable", problems)
        self.assertIn("supportReason-invalid-for-unavailable", problems)
        _, problems = acceptance.validate_recipe_read(dict(good, supportReason="original-missing"), PHOTO_ID)
        self.assertIn("supportReason-must-be-null-unless-unavailable", problems)

    def test_save_response_validation(self):
        _, problems = acceptance.validate_save_response(
            {"outcome": "saved", "recipeVersion": "rv-1", "sourceRevision": "src-1"}
        )
        self.assertEqual(problems, [])
        _, problems = acceptance.validate_save_response({"outcome": "saved"})
        self.assertIn("recipeVersion-missing-for-committed-outcome", problems)
        facts, _ = acceptance.validate_save_response(
            {
                "outcome": "recipe_conflict",
                "currentSourceRevision": "src-9",
                "currentRecipeVersion": "rv-7",
            }
        )
        self.assertEqual(facts["currentRecipeVersion"], "rv-7")

    def test_choose_exposure(self):
        controls = {
            "exposure": {"minimumEv": -5.0, "maximumEv": 5.0, "stepEv": 0.5}
        }
        self.assertEqual(acceptance.choose_exposure(controls, None), 0.5)
        self.assertEqual(acceptance.choose_exposure(controls, 0.5), 1.0)
        top = {"exposure": {"minimumEv": 0.0, "maximumEv": 0.5, "stepEv": 0.5}}
        self.assertEqual(acceptance.choose_exposure(top, 0.5), 0.0)
        narrow = {"exposure": {"minimumEv": 0.0, "maximumEv": 0.0, "stepEv": 0.5}}
        with self.assertRaises(acceptance.AcceptanceFailure) as caught:
            acceptance.choose_exposure(narrow, 0.0)
        self.assertEqual(caught.exception.reason, "no-exposure-headroom")
        self.assertEqual(acceptance.exposure_on_grid(controls, 0.3), 0.5)
        self.assertEqual(acceptance.exposure_on_grid(controls, None), 0.0)

    def test_build_save_body(self):
        body = acceptance.build_save_body("rid-1", None, "src-1", 0.5)
        self.assertEqual(
            body,
            {
                "requestId": "rid-1",
                "expectedRecipeVersion": None,
                "expectedSourceRevision": "src-1",
                "settings": {
                    "exposureEv": 0.5,
                    "whiteBalance": {"mode": "as-shot"},
                },
            },
        )

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

    def test_jpeg_walk(self):
        facts, problems = acceptance.validate_jpeg(minimal_jpeg(7, 5))
        self.assertEqual(problems, [])
        self.assertEqual(facts["width"], 7)
        self.assertEqual(facts["height"], 5)
        truncated = minimal_jpeg()[:-2]
        _, problems = acceptance.validate_jpeg(truncated)
        self.assertIn("jpeg-eoi-missing", problems)
        _, problems = acceptance.validate_jpeg(b"\x00\x01")
        self.assertIn("jpeg-soi-missing", problems)

    def test_header_framing_and_object_comparison(self):
        class Headers:
            def __init__(self, mapping):
                self.mapping = mapping

            def get_all(self, name):
                value = self.mapping.get(name)
                return [value] if value is not None else None

        fields = acceptance.ARTIFACT_METADATA_FIELDS
        metadata = {
            acceptance.ARTIFACT_METADATA_HEADERS[name]: str(index)
            for index, name in enumerate(fields)
        }
        collected, problems = acceptance.collect_metadata_headers(
            Headers(metadata), fields, acceptance.ARTIFACT_METADATA_HEADERS
        )
        self.assertEqual(problems, [])
        self.assertEqual(set(collected), set(fields))
        _, problems = acceptance.collect_metadata_headers(
            Headers({}), fields, acceptance.ARTIFACT_METADATA_HEADERS
        )
        self.assertEqual(len(problems), len(fields))
        artifact = {name: index for index, name in enumerate(fields)}
        self.assertEqual(acceptance.header_object_mismatches(collected, artifact), [])
        artifact["sha256"] = "different"
        problems = acceptance.header_object_mismatches(collected, artifact)
        self.assertEqual(problems, ["sha256-header-object-mismatch"])

    def test_artifact_object_validation(self):
        good = {
            "exportId": EXPORT_ID,
            "target": "development-tiff",
            "stage": "develop",
            "contentType": "image/tiff",
            "width": 4,
            "height": 3,
            "profileIdentity": "profile",
            "byteLength": 12,
            "sha256": "a" * 64,
            "expiresAt": "2026-01-01T00:00:00Z",
        }
        _, problems = acceptance.validate_artifact_object(good)
        self.assertEqual(problems, [])
        _, problems = acceptance.validate_artifact_object(dict(good, extra=1))
        self.assertIn("artifact-fields-not-exactly-closed-set", problems)
        _, problems = acceptance.validate_artifact_object(dict(good, expiresAt="soon"))
        self.assertIn("artifact-expiresAt-unparsable", problems)
        _, problems = acceptance.validate_artifact_object(dict(good, byteLength=10**30))
        self.assertIn("artifact-byteLength-exceeds-download-limit", problems)
        _, problems = acceptance.validate_artifact_object(dict(good, byteLength=0))
        self.assertIn("artifact-byteLength-not-positive", problems)

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

    def test_configured_download_bound_admits_a_full_resolution_artifact(self):
        # A full-resolution float32 Development TIFF is larger than the
        # default bound.  The deployment's own bound is what admits it, and
        # the launcher's hard output maximum is the ceiling.
        declared = 641_868_746
        _, problems = acceptance.artifact_download_limit(declared)
        self.assertEqual(problems, ["declared-byteLength-exceeds-download-limit"])
        limit, problems = acceptance.artifact_download_limit(
            declared, acceptance.MAXIMUM_DOWNLOAD_BYTES
        )
        self.assertEqual(problems, [])
        self.assertEqual(limit, declared + acceptance.DOWNLOAD_SLACK_BYTES)
        artifact = {
            "exportId": EXPORT_ID,
            "target": "development-tiff",
            "stage": "develop",
            "contentType": "image/tiff",
            "width": 6376,
            "height": 9568,
            "profileIdentity": "profile",
            "byteLength": declared,
            "sha256": "a" * 64,
            "expiresAt": "2026-01-01T00:00:00Z",
        }
        _, problems = acceptance.validate_artifact_object(artifact)
        self.assertEqual(problems, ["artifact-byteLength-exceeds-download-limit"])
        _, problems = acceptance.validate_artifact_object(
            artifact, acceptance.MAXIMUM_DOWNLOAD_BYTES
        )
        self.assertEqual(problems, [])

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

    def test_export_submission_binds_snapshot_identity(self):
        good = {
            "exportId": EXPORT_ID,
            "state": "queued",
            "target": "development-tiff",
            "recipeVersion": "rv-2",
            "sourceRevision": "src-1",
            "receiptExpiresAt": None,
            "artifactExpiresAt": None,
        }
        _, problems = acceptance.validate_export_submission(good, "rv-2", "src-1")
        self.assertEqual(problems, [])
        _, problems = acceptance.validate_export_submission(
            dict(good, recipeVersion="rv-1", sourceRevision="src-9"), "rv-2", "src-1"
        )
        self.assertIn("recipeVersion-mismatch", problems)
        self.assertIn("sourceRevision-mismatch", problems)
        _, problems = acceptance.validate_export_submission(dict(good, exportId="../../evil"))
        self.assertIn("exportId-outside-character-set", problems)

    def test_export_inspection_binds_photo_and_snapshot_identity(self):
        base = {
            "exportId": EXPORT_ID,
            "photoId": PHOTO_ID,
            "state": "succeeded",
            "target": "development-tiff",
            "recipeVersion": "rv-2",
            "sourceRevision": "src-1",
            "bundleId": BUNDLE_ID,
            "terminalOutcome": "succeeded",
            "failureReason": None,
            "receiptExpiresAt": "2026-01-01T00:00:00Z",
            "artifact": None,
        }
        _, problems = acceptance.validate_export_inspection(
            base, EXPORT_ID, photo_id=PHOTO_ID, recipe_version="rv-2", source_revision="src-1"
        )
        self.assertEqual(
            [problem for problem in problems if problem != "artifact-null-after-succeeded"],
            [],
        )
        foreign = dict(base, photoId="other-photo", recipeVersion="rv-1", sourceRevision="src-9")
        _, problems = acceptance.validate_export_inspection(
            foreign, EXPORT_ID, photo_id=PHOTO_ID, recipe_version="rv-2", source_revision="src-1"
        )
        self.assertIn("photoId-mismatch", problems)
        self.assertIn("recipeVersion-mismatch", problems)
        self.assertIn("sourceRevision-mismatch", problems)

    def test_export_inspection_validation(self):
        base = {
            "exportId": EXPORT_ID,
            "photoId": PHOTO_ID,
            "state": "running",
            "target": "development-tiff",
            "recipeVersion": "rv-1",
            "sourceRevision": "src-1",
            "bundleId": BUNDLE_ID,
            "terminalOutcome": None,
            "failureReason": None,
            "receiptExpiresAt": None,
            "artifact": None,
        }
        _, problems = acceptance.validate_export_inspection(base, EXPORT_ID)
        self.assertEqual(problems, [])
        succeeded = dict(
            base,
            state="succeeded",
            terminalOutcome="succeeded",
            receiptExpiresAt="2026-01-01T00:00:00Z",
        )
        _, problems = acceptance.validate_export_inspection(succeeded, EXPORT_ID)
        self.assertIn("artifact-null-after-succeeded", problems)
        failed = dict(
            base,
            state="failed",
            terminalOutcome="failed",
            receiptExpiresAt="2026-01-01T00:00:00Z",
        )
        _, problems = acceptance.validate_export_inspection(failed, EXPORT_ID)
        self.assertIn("failureReason-missing-after-failure", problems)
        _, problems = acceptance.validate_export_inspection(
            dict(base, state="queued", terminalOutcome="succeeded"), EXPORT_ID
        )
        self.assertIn("terminalOutcome-inconsistent-with-state", problems)

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
        with self.assertRaises(acceptance.InvocationRefused):
            acceptance.normalize_base_url("http://photos.example.com")
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


class RefusalTests(AcceptanceTestCase):
    def test_refuses_without_acknowledgement(self):
        with RunningStub(StubDeployment()) as stub:
            argv = [
                "--base-url", stub.base_url,
                "--token-file", str(self.token_file),
                "--fixture", str(self.fixture),
                "--output-dir", str(self.output_dir),
            ]
            code, report, message = run_main(argv)
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "refused")
        self.assertEqual(report["reason"], "acceptance-instance-not-acknowledged")
        self.assertIn("Refusing to run", message)

    def test_refuses_missing_fixture(self):
        with RunningStub(StubDeployment()) as stub:
            argv = self.invocation(stub)
            argv[argv.index("--fixture") + 1] = str(self.root / "missing.dng")
            code, report, _ = run_main(argv)
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "refused")


class DryRunTests(AcceptanceTestCase):
    def test_full_workflow_passes_against_stub(self):
        stub = StubDeployment(preview_202_first=True)
        with RunningStub(stub) as running:
            code, report, summary = run_main(self.invocation(running))
        self.assertEqual(code, 0, report)
        self.assertEqual(report["status"], "passed")
        statuses = {step["name"]: step["status"] for step in report["steps"]}
        self.assertEqual(
            statuses,
            {
                "capability": "pass",
                "resolve-photo": "pass",
                "read-recipe": "pass",
                "save-exposure": "pass",
                "save-undo": "pass",
                "edit-preview": "pass",
                "submit-export": "pass",
                "export-settlement": "pass",
                "download-artifact": "pass",
                "film-stage": "skipped",
                "original-invariance": "pass",
            },
        )
        self.assertEqual(report["filmStage"], {
            "covered": False,
            "ownerIssue": 332,
            "observedCapabilityStage": "unavailable",
        })
        identities = report["identities"]
        self.assertEqual(identities["capabilityState"], "ready")
        self.assertEqual(identities["bundleId"], BUNDLE_ID)
        self.assertEqual(identities["photoId"], PHOTO_ID)
        self.assertEqual(identities["recipeVersionAfterSave"], "rv-1")
        self.assertEqual(identities["recipeVersionAfterUndo"], "rv-2")
        self.assertEqual(identities["exportState"], "succeeded")
        artifact = identities["artifact"]
        self.assertEqual(
            artifact["sha256"], hashlib.sha256(stub.artifact_bytes).hexdigest()
        )
        self.assertEqual(artifact["width"], 4)
        self.assertEqual(artifact["height"], 3)
        self.assertEqual(artifact["profileIdentity"], hashlib.sha256(PROFILE_ASSET.read_bytes()).hexdigest())
        self.assertEqual(report["notRun"][0]["step"], "film-stage")
        self.assertEqual(report["notRun"][0]["reason"], "film-stage-not-implemented")
        self.assertIn("Film (finished-jpeg) stage not covered", summary)
        self.assertIn("film-stage-not-implemented", summary)
        downloaded = Path(report["writtenFiles"][0])
        self.assertTrue(downloaded.is_file())
        self.assertEqual(downloaded.read_bytes(), stub.artifact_bytes)
        self.assertEqual(downloaded.parent, self.output_dir.resolve())
        self.assertEqual(report["fixture"]["sha256"], hashlib.sha256(FIXTURE_BYTES).hexdigest())
        # Every stub request carried the bearer token and CLI contract header;
        # the report lists the exact commands in order.
        self.assertGreater(report["counters"]["requests"], 8)
        methods = [entry["method"] for entry in report["steps"][0]["requests"]]
        self.assertEqual(methods, ["GET"])

    def test_export_failure_fails_the_run(self):
        with RunningStub(StubDeployment(fail_export=True)) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        self.assertEqual(report["status"], "failed")
        failed = {step["name"] for step in report["steps"] if step["status"] == "fail"}
        self.assertIn("export-settlement", failed)
        settlement = next(step for step in report["steps"] if step["name"] == "export-settlement")
        self.assertEqual(settlement["reason"], "export-not-succeeded")
        self.assertEqual(settlement["detail"]["state"], "failed")
        skipped = {step["name"] for step in report["steps"] if step["status"] == "skipped"}
        self.assertIn("download-artifact", skipped)

    def test_not_ready_capability_blocks_processing_steps(self):
        with RunningStub(StubDeployment(capability_state="bundle-unavailable", develop_stage="unavailable")) as stub:
            code, report, summary = run_main(self.invocation(stub))
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "blocked")
        skipped = {step["name"]: step for step in report["steps"] if step["status"] == "skipped"}
        self.assertIn("edit-preview", skipped)
        self.assertIn("submit-export", skipped)
        self.assertEqual(skipped["edit-preview"]["reason"], "develop-stage-not-ready")
        self.assertEqual(skipped["edit-preview"]["detail"]["capabilityState"], "bundle-unavailable")
        # Guarded saves do not need the engine; they still ran.
        self.assertEqual(next(step for step in report["steps"] if step["name"] == "save-undo")["status"], "pass")
        self.assertIn("Could not run edit-preview", summary)
        self.assertEqual(report["counters"]["stepsFailed"], 0)

    def test_missing_preview_route_is_reported_not_run(self):
        with RunningStub(StubDeployment(disabled_routes=("edit-preview",))) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "blocked")
        preview = next(step for step in report["steps"] if step["name"] == "edit-preview")
        self.assertEqual(preview["status"], "skipped")
        self.assertEqual(preview["reason"], "route-not-deployed")
        # The export chain is independent of the preview route and still passes.
        self.assertEqual(next(step for step in report["steps"] if step["name"] == "download-artifact")["status"], "pass")

    def test_missing_capability_route_cascades(self):
        with RunningStub(StubDeployment(disabled_routes=("capability",))) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 2)
        self.assertEqual(report["status"], "blocked")
        statuses = {step["name"]: (step["status"], step.get("reason")) for step in report["steps"]}
        self.assertEqual(statuses["capability"], ("skipped", "route-not-deployed"))
        self.assertEqual(statuses["resolve-photo"], ("skipped", "prerequisite-not-passed"))
        self.assertEqual(statuses["download-artifact"], ("skipped", "prerequisite-not-passed"))
        # Invariance hashing always runs, even when the workflow cannot.
        self.assertEqual(statuses["original-invariance"], ("pass", None))

    def test_preview_render_timeout_fails_the_run(self):
        stub = StubDeployment(preview_202_always=True)
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running, ["--preview-timeout", "0.05"]))
        self.assertEqual(code, 1)
        preview = next(step for step in report["steps"] if step["name"] == "edit-preview")
        self.assertEqual(preview["status"], "fail")
        self.assertEqual(preview["reason"], "preview-render-timeout")
        self.assertGreater(preview["detail"]["polls"], 0)

    def test_wrong_token_fails_with_auth_reason(self):
        with RunningStub(StubDeployment(wrong_token=True)) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        capability = next(step for step in report["steps"] if step["name"] == "capability")
        self.assertEqual(capability["status"], "fail")
        self.assertEqual(capability["reason"], "capability-refused")
        self.assertEqual(capability["detail"]["status"], 401)

    def test_ambiguous_fixture_photo_fails(self):
        with RunningStub(StubDeployment(duplicate_fixture=True)) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        resolve = next(step for step in report["steps"] if step["name"] == "resolve-photo")
        self.assertEqual(resolve["reason"], "fixture-photo-ambiguous")

    def test_invariance_detects_fixture_mutation(self):
        stub = StubDeployment()

        original_handle = stub.handle

        def mutating_handle(method, path, body, headers):
            if path == f"/api/photos/{PHOTO_ID}/edit-preview/develop":
                self.fixture.write_bytes(b"mutated in flight")
            return original_handle(method, path, body, headers)

        stub.handle = mutating_handle
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running))
        self.assertEqual(code, 1)
        invariance = next(step for step in report["steps"] if step["name"] == "original-invariance")
        self.assertEqual(invariance["status"], "fail")
        self.assertEqual(invariance["reason"], "original-mutated")
        self.assertTrue(
            any(change.startswith("sha256-changed:") for change in invariance["detail"]["changes"])
        )

    def test_photo_query_speaks_the_server_contract(self):
        """The runner sends the tagged source object within the published bound."""
        stub = StubDeployment()
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running))
        self.assertEqual(code, 0, report)
        query_requests = [
            entry
            for entry in stub.requests
            if entry["path"] == "/api/photo-queries" and entry["method"] == "POST"
        ]
        self.assertEqual(len(query_requests), 1)
        body = query_requests[0]["body"]
        self.assertEqual(
            body,
            {"source": {"kind": "all"}, "kind": "raw", "available": True, "limit": 60},
        )
        capabilities_requests = [
            entry for entry in stub.requests if entry["path"] == "/api/capabilities"
        ]
        self.assertEqual(len(capabilities_requests), 1)
        resolve = next(step for step in report["steps"] if step["name"] == "resolve-photo")
        self.assertEqual(resolve["detail"]["listPageMaximum"], 60)

    def test_photo_query_pages_with_published_limit(self):
        """The runner honors `limits.listPageMaximum` and follows the cursor."""
        later_photo = dict(
            StubDeployment().photos[0],
            id="aaaaaaaa-0000-4000-8000-00000000beef",
            filename="OTHER_0001.ARW",
        )
        fixture_photo = StubDeployment().photos[0]
        stub = StubDeployment(
            list_page_maximum=2,
            query_pages=[
                [later_photo, dict(later_photo, id="bbbbbbbb-0000-4000-8000-00000000cafe")],
                [fixture_photo],
            ],
        )
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running))
        self.assertEqual(code, 0, report)
        query_requests = [
            entry
            for entry in stub.requests
            if entry["path"] == "/api/photo-queries" and entry["method"] == "POST"
        ]
        self.assertEqual(query_requests[0]["body"]["limit"], 2)
        cursor_requests = [
            entry
            for entry in stub.requests
            if entry["path"].startswith("/api/photo-queries/query-cursor")
        ]
        self.assertEqual(len(cursor_requests), 1)
        resolve = next(step for step in report["steps"] if step["name"] == "resolve-photo")
        self.assertEqual(resolve["detail"]["listPageMaximum"], 2)
        self.assertEqual(resolve["detail"]["photoId"], PHOTO_ID)
        self.assertEqual(report["identities"]["photoId"], PHOTO_ID)

    def test_sidecar_is_snapshot_and_proved_unchanged(self):
        sidecar = self.fixture.with_name(self.fixture.name + ".xmp")
        sidecar.write_bytes(b"<x/>")
        with RunningStub(StubDeployment()) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 0)
        self.assertEqual(len(report["sidecars"]), 1)
        self.assertEqual(report["sidecars"][0]["sha256"], hashlib.sha256(b"<x/>").hexdigest())
        self.assertIn(str(sidecar), report["sidecars"][0]["path"])

    def test_download_refuses_symlinked_artifact_destination(self):
        self.output_dir.mkdir(parents=True)
        escape = self.output_dir / f"{EXPORT_ID}.tiff"
        escape.symlink_to(self.root / "outside" / "stolen.tiff")
        with RunningStub(StubDeployment()) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        download = next(step for step in report["steps"] if step["name"] == "download-artifact")
        self.assertEqual(download["reason"], "artifact-path-symlink")
        self.assertFalse(escape.exists())
        self.assertNotIn(
            str(escape), report.get("writtenFiles", [])
        )

    def test_settlement_rejects_foreign_photo_inspection(self):
        with RunningStub(StubDeployment(inspect_photo_id="ffffffff-0000-4000-8000-00000000dead")) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        settlement = next(step for step in report["steps"] if step["name"] == "export-settlement")
        self.assertEqual(settlement["reason"], "export-inspect-invalid")
        self.assertEqual(settlement["detail"]["problems"], ["photoId-mismatch"])

    def test_submission_rejects_stale_recipe_version(self):
        with RunningStub(StubDeployment(submit_recipe_version="rv-stale")) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        submit = next(step for step in report["steps"] if step["name"] == "submit-export")
        self.assertEqual(submit["reason"], "export-submit-invalid")
        self.assertIn("recipeVersion-mismatch", submit["detail"]["problems"])
        skipped = {step["name"] for step in report["steps"] if step["status"] == "skipped"}
        self.assertIn("export-settlement", skipped)

    def test_artifact_rejects_foreign_export_id_in_metadata(self):
        with RunningStub(StubDeployment(artifact_export_id="another-export-2")) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        settlement = next(step for step in report["steps"] if step["name"] == "export-settlement")
        self.assertIn("artifact-exportId-mismatch", settlement["detail"]["problems"])

    def test_preview_rejects_wrong_content_type(self):
        with RunningStub(StubDeployment(preview_content_type="text/plain")) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        preview = next(step for step in report["steps"] if step["name"] == "edit-preview")
        self.assertEqual(preview["reason"], "preview-metadata-invalid")
        self.assertIn("header-contentType-unsupported", preview["detail"]["problems"])

    def test_preview_rejects_nonjpeg_body_with_jpeg_content_type(self):
        with RunningStub(StubDeployment(preview_body_override=b"not a jpeg at all")) as stub:
            code, report, _ = run_main(self.invocation(stub))
        self.assertEqual(code, 1)
        preview = next(step for step in report["steps"] if step["name"] == "edit-preview")
        self.assertEqual(preview["reason"], "preview-body-invalid")
        self.assertIn("preview-body-contentType-mismatch", preview["detail"]["problems"])

    def test_unreadable_sidecar_fails_with_json_report_not_traceback(self):
        stub = StubDeployment()
        sidecar = self.fixture.with_name(self.fixture.name + ".xmp")
        sidecar.write_bytes(b"<x/>")

        original_handle = stub.handle

        def sabotaging_handle(method, path, body, headers):
            if path == f"/api/photos/{PHOTO_ID}/edit-preview/develop":
                sidecar.unlink()
                sidecar.mkdir()
            return original_handle(method, path, body, headers)

        stub.handle = sabotaging_handle
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running))
        self.assertEqual(code, 1)
        invariance = next(step for step in report["steps"] if step["name"] == "original-invariance")
        self.assertEqual(report["status"], "failed")
        self.assertEqual(invariance["reason"], "invariance-snapshot-failed")
        self.assertEqual(invariance["detail"]["unreadable"][0]["path"], str(sidecar))
        self.assertEqual(invariance["detail"]["unreadable"][0]["error"], "IsADirectoryError")

    def test_artifact_rejects_invalid_deflate_payload(self):
        invalid = build_development_tiff(
            4, 3, PROFILE_ASSET.read_bytes(), deflate=False
        )
        stub = StubDeployment(artifact_bytes_override=invalid)
        with RunningStub(stub) as running:
            code, report, _ = run_main(self.invocation(running))
        self.assertEqual(code, 1)
        download = next(
            step for step in report["steps"] if step["name"] == "download-artifact"
        )
        self.assertEqual(download["reason"], "artifact-invalid")
        self.assertIn("tiff-deflate-strip-invalid", download["detail"]["problems"])
        self.assertEqual(report["writtenFiles"], [])


if __name__ == "__main__":
    unittest.main()
