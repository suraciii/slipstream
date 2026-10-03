"""Selected darktable step operator acceptance for Issue #496.

Run only against an acknowledged dedicated acceptance instance and an explicitly
approved RAW fixture. Qualification remains limited to the pinned engine,
fixture, manual exposure tree, output contract and finite deployment allocation.
Standalone SpektraFilm qualification is a separate explicitly admitted exercise.
"""
from __future__ import annotations
import argparse
import copy
import hashlib
import json
import math
import os
import re
import secrets
import stat
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

MAX_JSON_BYTES = 1024 * 1024
MAX_DOWNLOAD_BYTES = 512 * 1024 * 1024
MAXIMUM_DOWNLOAD_BYTES = 4 * 1024 * 1024 * 1024
DOWNLOAD_SLACK_BYTES = 65536
STRIP_PADDING_MAXIMUM = 64
MAX_TOKEN_BYTES = 4096
MAX_QUERY_PAGES = 50
FALLBACK_LIST_PAGE_MAXIMUM = 60
MAXIMUM_QUERY_LIMIT_BOUND = 10_000
CAPABILITIES_PATH = "/api/capabilities"
PHOTO_QUERIES_PATH = "/api/photo-queries"
MODULES_PATH = "/api/processing/modules"
RECIPE_PATH = "/api/photos/{id}/processing-recipe"
PREVIEW_PATH = "/api/photos/{id}/processing-preview/{step}"
EXPORTS_PATH = "/api/photos/{id}/processing-exports"
ARTIFACT_PATH = "/api/processing-artifacts/{id}"
PINNED_SOURCE_PROFILE_DIGESTS = (
    "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed",
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe",
)
_LOWER_HEX_64 = re.compile(r"\A[0-9a-f]{64}\Z")
_REQUEST_IDENTITY = re.compile(r"\A[A-Za-z0-9._-]{1,128}\Z")

class AcceptanceFailure(Exception):
    """A step failed for the recorded reason."""

    def __init__(self, reason: str, detail: dict | None = None):
        super().__init__(reason)
        self.reason = reason
        self.detail = detail or {}



class TransportFailure(Exception):
    """The HTTP transport itself failed (unreachable, timeout, reset)."""

    def __init__(self, reason: str, detail: dict | None = None):
        super().__init__(reason)
        self.reason = reason
        self.detail = detail or {}


class InvocationRefused(Exception):
    """The invocation was refused before any step ran."""

    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


# ---------------------------------------------------------------------------
# Pure helpers (unit-tested).
# ---------------------------------------------------------------------------


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def valid_request_identity(value: str) -> bool:
    return isinstance(value, str) and _REQUEST_IDENTITY.match(value) is not None


def new_request_identity(purpose: str) -> str:
    return f"acceptance-{purpose}-{secrets.token_hex(6)}"


def parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str) or not value.strip():
        return None
    text = value.strip()
    if text.endswith(("Z", "z")):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def _require_string(payload: dict, key: str, problems: list) -> str | None:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        problems.append(f"{key}-missing-or-not-string")
        return None
    return value


def qualified_darktable_tree(exposure: float) -> dict:
    return {"stack": [{"operation": "exposure", "multiPriority": 0,
        "enabled": True, "params": {"mode": "EXPOSURE_MODE_MANUAL",
        "black": 0.0, "exposure": exposure, "compensate_exposure_bias": False,
        "compensate_hilite_pres": False}}], "output": {"format": "tiff",
        "precisionBits": 32, "colorSpace": "prophoto-rgb",
        "transferFunction": "linear", "geometry": "source-preserving"}}


def validate_recipe_read(payload: object, photo_id: str) -> tuple[dict, list]:
    problems = []
    if not isinstance(payload, dict):
        return {}, ["payload-not-object"]
    if payload.get("photoId") != photo_id:
        problems.append("photoId-mismatch")
    source = payload.get("sourceRevision")
    if not isinstance(source, str) or not source or len(source.encode("utf-8")) > 16384:
        problems.append("sourceRevision-invalid")
    recipe = payload.get("recipe")
    if recipe is not None:
        if not isinstance(recipe, dict):
            problems.append("recipe-not-object")
        else:
            if recipe.get("photoId") != photo_id:
                problems.append("recipe-photoId-mismatch")
            revision = recipe.get("revision")
            if not isinstance(revision, str) or not revision or len(revision.encode("utf-8")) > 128:
                problems.append("recipe-revision-missing-or-outside-bound")
            if recipe.get("sourceRevision") != source:
                problems.append("recipe-requires-explicit-rebind")
            steps = recipe.get("steps")
            if not isinstance(steps, list):
                problems.append("recipe-steps-not-array")
            elif any(not isinstance(step, dict) or not isinstance(step.get("stepId"), str)
                     or not step["stepId"] for step in steps):
                problems.append("recipe-step-invalid")
            elif recipe.get("currentStepId") is not None and sum(
                step.get("stepId") == recipe["currentStepId"] for step in steps) != 1:
                problems.append("recipe-current-step-invalid")
    return dict(payload), problems


def validate_captured_identity(value: object, expected: dict) -> list:
    if not isinstance(value, dict):
        return ["captured-identity-not-object"]
    return [f"{key}-mismatch" for key, wanted in expected.items() if value.get(key) != wanted]


def validate_png(data: bytes, maximum_edge: int) -> tuple[dict, list]:
    """Inspect complete PNG framing, CRCs, dimensions and bounded decoded rows."""
    facts, problems = {}, []
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return facts, ["png-signature-invalid"]
    position, compressed, seen_end = 8, bytearray(), False
    while position < len(data):
        if position + 12 > len(data):
            return facts, ["png-chunk-truncated"]
        size = struct.unpack(">I", data[position:position+4])[0]
        kind = data[position+4:position+8]
        end = position + 12 + size
        if end > len(data):
            return facts, ["png-chunk-truncated"]
        payload = data[position+8:end-4]
        if zlib.crc32(kind + payload) & 0xffffffff != struct.unpack(">I", data[end-4:end])[0]:
            return facts, ["png-crc-invalid"]
        if kind == b"IHDR":
            if position != 8 or size != 13 or facts:
                return facts, ["png-ihdr-invalid"]
            width, height, bits, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            if not 0 < width <= maximum_edge or not 0 < height <= maximum_edge:
                return facts, ["png-geometry-exceeds-bound"]
            if bits != 8 or color not in (2, 6) or (compression, filtering, interlace) != (0, 0, 0):
                return facts, ["png-pixel-contract-invalid"]
            facts = {"width": width, "height": height, "channels": 3 if color == 2 else 4}
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            if size or end != len(data):
                return facts, ["png-end-invalid"]
            seen_end = True
        position = end
    if not facts or not seen_end:
        return facts, ["png-incomplete"]
    expected = (facts["width"] * facts["channels"] + 1) * facts["height"]
    decoder = zlib.decompressobj()
    try:
        decoded = decoder.decompress(compressed, expected + 1)
    except zlib.error:
        return facts, ["png-deflate-invalid"]
    if len(decoded) != expected or not decoder.eof or decoder.unused_data or decoder.unconsumed_tail:
        problems.append("png-decoded-size-invalid")
    elif any(decoded[row * (facts["width"] * facts["channels"] + 1)] > 4 for row in range(facts["height"])):
        problems.append("png-filter-invalid")
    return facts, problems


def artifact_download_limit(declared: object, maximum: int = MAX_DOWNLOAD_BYTES) -> tuple[int, list]:
    """Clamp the read limit for a download to the configured maximum.

    Returns the applied byte limit and problems for an unusable declared
    size.  A declared size above `maximum` is refused instead of being
    clamped, because accepting it would mean reading an artifact the
    deployment cannot legitimately publish.
    """
    problems: list = []
    if not isinstance(declared, int) or isinstance(declared, bool):
        problems.append("declared-byteLength-not-integer")
        return maximum, problems
    if declared <= 0:
        problems.append("declared-byteLength-not-positive")
        return maximum, problems
    if declared > maximum:
        problems.append("declared-byteLength-exceeds-download-limit")
        return maximum, problems
    return min(declared + DOWNLOAD_SLACK_BYTES, maximum), problems


_TIFF_TYPE_SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}
_MAXIMUM_TIFF_BYTES = MAXIMUM_DOWNLOAD_BYTES
_MAXIMUM_TIFF_PIXELS = 200_000_000
_MAXIMUM_TIFF_ENTRIES = 512
_MAXIMUM_TIFF_VALUE_BYTES = 1024 * 1024
_MAXIMUM_TIFF_STRIPS = 65_536
_TIFF_DECODE_CHUNK_BYTES = 64 * 1024


def _deflate_strip_matches(data: bytes, offset: int, length: int, expected: int) -> bool:
    """Validate one zlib-wrapped strip without retaining decoded pixels."""
    decoder = zlib.decompressobj()
    decoded = 0
    end = offset + length
    try:
        for start in range(offset, end, _TIFF_DECODE_CHUNK_BYTES):
            if decoder.eof:
                return False
            pending = data[start : min(start + _TIFF_DECODE_CHUNK_BYTES, end)]
            while pending:
                limit = min(_TIFF_DECODE_CHUNK_BYTES, expected - decoded + 1)
                output = decoder.decompress(pending, limit)
                decoded += len(output)
                if decoded > expected or decoder.unused_data:
                    return False
                pending = decoder.unconsumed_tail
                if not pending:
                    break
                if not output:
                    return False
    except zlib.error:
        return False
    return decoder.eof and decoded == expected and not decoder.unused_data


def validate_development_tiff(
    data: bytes,
    expected_width: int | None = None,
    expected_height: int | None = None,
    accepted_profile_digests: tuple[str, ...] = PINNED_SOURCE_PROFILE_DIGESTS,
) -> tuple[dict, list]:
    """Validate TIFF framing and inflate every bounded Deflate strip."""
    problems: list = []
    facts: dict = {"decode": "ifd-and-deflate-strips"}
    if len(data) < 8:
        return facts, ["tiff-truncated-header"]
    if len(data) > _MAXIMUM_TIFF_BYTES:
        return facts, ["tiff-size-exceeds-maximum"]
    byte_order = data[:2]
    endian = "<" if byte_order == b"II" else ">" if byte_order == b"MM" else None
    if endian is None:
        return facts, ["tiff-byte-order-invalid"]
    if struct.unpack(endian + "H", data[2:4])[0] != 42:
        return facts, ["tiff-magic-invalid"]

    ifd_offset = struct.unpack(endian + "I", data[4:8])[0]
    if ifd_offset + 2 > len(data):
        return facts, ["tiff-ifd-out-of-range"]
    entry_count = struct.unpack(endian + "H", data[ifd_offset : ifd_offset + 2])[0]
    if entry_count == 0 or entry_count > _MAXIMUM_TIFF_ENTRIES:
        return facts, ["tiff-ifd-entry-count-out-of-range"]
    ifd_end = ifd_offset + 2 + 12 * entry_count + 4
    if ifd_end > len(data):
        return facts, ["tiff-ifd-out-of-range"]

    entries: dict[int, tuple[int, int, bytes, int]] = {}
    value_ranges: list[tuple[int, int]] = []
    for index in range(entry_count):
        start = ifd_offset + 2 + 12 * index
        tag, kind, count = struct.unpack(endian + "HHI", data[start : start + 8])
        type_size = _TIFF_TYPE_SIZES.get(kind)
        if type_size is None:
            continue
        byte_count = type_size * count
        if byte_count <= 4:
            value_offset = start + 8
        else:
            value_offset = struct.unpack(endian + "I", data[start + 8 : start + 12])[0]
        value_end = value_offset + byte_count
        if value_end > len(data):
            problems.append(f"tiff-tag-{tag}-value-out-of-range")
            continue
        value_ranges.append((value_offset, value_end))
        if tag in (273, 279):
            maximum = _MAXIMUM_TIFF_STRIPS * 4
        elif tag == 34675:
            maximum = _MAXIMUM_TIFF_VALUE_BYTES
        elif tag in (256, 257, 258, 259, 262, 273, 274, 277, 278, 279, 339):
            maximum = 64
        else:
            # Engine metadata such as XMP and private tags is part of the
            # bounded file, but is not part of the closed pixel contract.
            # Account for its range without retaining an unbounded value.
            maximum = None
        if maximum is not None and byte_count > maximum:
            problems.append(f"tiff-tag-{tag}-value-exceeds-bound")
            continue
        raw = data[value_offset:value_end]
        entries[tag] = (kind, count, raw, value_offset)
    def unsigned(tag: int) -> int | None:
        entry = entries.get(tag)
        if entry is None:
            problems.append(f"tiff-tag-{tag}-missing")
            return None
        kind, count, raw, _ = entry
        if kind == 3 and count == 1 and len(raw) >= 2:
            return struct.unpack(endian + "H", raw[:2])[0]
        if kind == 4 and count == 1 and len(raw) >= 4:
            return struct.unpack(endian + "I", raw[:4])[0]
        problems.append(f"tiff-tag-{tag}-unexpected-type")
        return None

    def integer_list(tag: int, expected_count: int | None = None) -> list[int] | None:
        entry = entries.get(tag)
        if entry is None:
            problems.append(f"tiff-tag-{tag}-missing")
            return None
        kind, count, raw, _ = entry
        if kind not in (3, 4) or (expected_count is not None and count != expected_count):
            problems.append(f"tiff-tag-{tag}-unexpected-shape")
            return None
        width = 2 if kind == 3 else 4
        if len(raw) != width * count:
            problems.append(f"tiff-tag-{tag}-unexpected-shape")
            return None
        code = "H" if kind == 3 else "I"
        return list(struct.unpack(endian + code * count, raw))

    width = unsigned(256)
    height = unsigned(257)
    bits = integer_list(258, 3)
    compression = unsigned(259)
    photometric = unsigned(262)
    samples = unsigned(277)
    rows_per_strip = unsigned(278)
    sample_format = integer_list(339, 3)
    facts.update(
        {
            "width": width,
            "height": height,
            "bitsPerSample": bits,
            "sampleFormat": sample_format,
            "compression": compression,
            "photometricInterpretation": photometric,
            "samplesPerPixel": samples,
        }
    )
    if width is None or height is None:
        return facts, problems
    if width == 0 or height == 0 or width * height > _MAXIMUM_TIFF_PIXELS:
        problems.append("tiff-dimensions-out-of-range")
    if samples != 3:
        problems.append("tiff-samples-per-pixel-not-3")
    if bits != [32, 32, 32]:
        problems.append("tiff-bits-per-sample-not-float32-rgb")
    if sample_format != [3, 3, 3]:
        problems.append("tiff-sample-format-not-ieee-float")
    # The closed Development TIFF contract is IEEE float32 RGB samples with
    # Deflate strip ranges, and the service refuses anything else before it
    # publishes.
    if compression != 8:
        problems.append("tiff-compression-not-deflate")
    if photometric != 2:
        problems.append("tiff-photometric-not-rgb")
    if expected_width is not None and width != expected_width:
        problems.append("tiff-width-mismatch")
    if expected_height is not None and height != expected_height:
        problems.append("tiff-height-mismatch")

    profile_entry = entries.get(34675)
    if profile_entry is None:
        problems.append("tiff-embedded-profile-missing")
        facts["profileSha256"] = None
    else:
        profile_bytes = profile_entry[2]
        digest = sha256_hex(profile_bytes)
        facts["profileSha256"] = digest
        facts["profileAccepted"] = digest in accepted_profile_digests
        if digest not in accepted_profile_digests:
            problems.append("tiff-embedded-profile-digest-not-pinned")

    if (
        width == 0 or height == 0 or width * height > _MAXIMUM_TIFF_PIXELS
        or (expected_width is not None and width != expected_width)
        or (expected_height is not None and height != expected_height)
    ):
        return facts, problems
    if compression != 8 or photometric != 2 or bits != [32, 32, 32] or sample_format != [3, 3, 3]:
        return facts, problems
    if samples != 3 or rows_per_strip is None or rows_per_strip == 0:
        return facts, problems
    offsets = integer_list(273)
    byte_counts = integer_list(279)
    if offsets is None or byte_counts is None:
        return facts, problems
    expected_strips = (height + rows_per_strip - 1) // rows_per_strip
    if (
        expected_strips == 0 or expected_strips > _MAXIMUM_TIFF_STRIPS
        or len(offsets) != expected_strips or len(byte_counts) != expected_strips
    ):
        problems.append("tiff-strip-layout-shape-invalid")
        return facts, problems
    covered_end = max(
        ifd_end,
        max((end for _, end in value_ranges), default=ifd_end),
    )
    decoded_bytes = 0
    for index, (offset, byte_count) in enumerate(zip(offsets, byte_counts)):
        rows = min(rows_per_strip, height - index * rows_per_strip)
        expected_bytes = rows * width * samples * 4
        end = offset + byte_count
        if byte_count == 0 or offset < covered_end or end > len(data):
            problems.append("tiff-strip-range-invalid")
            return facts, problems
        covered_end = end
        if not _deflate_strip_matches(data, offset, byte_count, expected_bytes):
            problems.append("tiff-deflate-strip-invalid")
            return facts, problems
        decoded_bytes += expected_bytes
    facts["decodedBytes"] = decoded_bytes
    if len(data) - covered_end > STRIP_PADDING_MAXIMUM:
        problems.append("tiff-unaccounted-trailing-payload")
    return facts, problems

def snapshot_original(path: Path) -> dict:
    """Read-only identity snapshot of one Original or sidecar file."""
    metadata = path.stat()
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return {
        "path": str(path),
        "byteLength": metadata.st_size,
        "sha256": digest.hexdigest(),
        "mtimeNs": metadata.st_mtime_ns,
        "mode": oct(stat.S_IMODE(metadata.st_mode)),
    }


def external_xmp_sidecars(fixture: Path) -> list[Path]:
    """External XMP sidecar paths conventionally paired with the fixture."""
    candidates = [fixture.with_name(fixture.name + ".xmp"), fixture.with_suffix(".xmp")]
    seen: set = set()
    sidecars: list[Path] = []
    for candidate in candidates:
        resolved = str(candidate)
        if resolved not in seen:
            seen.add(resolved)
            sidecars.append(candidate)
    return sidecars


def invariance_changes(before: dict, after: dict) -> list:
    problems = []
    if set(before) != set(after):
        for path in sorted(set(before) - set(after)):
            problems.append(f"file-disappeared:{path}")
        for path in sorted(set(after) - set(before)):
            problems.append(f"file-appeared:{path}")
        return problems
    for path in sorted(before):
        before_snapshot = before[path]
        after_snapshot = after[path]
        for key in ("byteLength", "sha256", "mtimeNs", "mode"):
            if before_snapshot[key] != after_snapshot[key]:
                problems.append(f"{key}-changed:{path}")
    return problems


def normalize_base_url(raw: str) -> str:
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme not in ("http", "https"):
        raise InvocationRefused("base-url-scheme-must-be-http-or-https")
    if parsed.username is not None or parsed.password is not None:
        raise InvocationRefused("base-url-must-not-carry-credentials")
    if not parsed.hostname:
        raise InvocationRefused("base-url-host-missing")
    if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise InvocationRefused("base-url-must-not-carry-path-query-or-fragment")
    port = f":{parsed.port}" if parsed.port else ""
    return f"{parsed.scheme}://{parsed.hostname}{port}"


def read_token_file(path: Path) -> str:
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise InvocationRefused("token-file-not-regular")
        if metadata.st_mode & 0o022:
            raise InvocationRefused("token-file-writable-by-group-or-others")
        with path.open("rb") as stream:
            raw_bytes = stream.read(MAX_TOKEN_BYTES + 1)
    except InvocationRefused:
        raise
    except OSError as error:
        raise InvocationRefused("token-file-unreadable") from error
    if len(raw_bytes) > MAX_TOKEN_BYTES:
        raise InvocationRefused("token-file-too-large")
    try:
        raw = raw_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise InvocationRefused("token-file-invalid") from error
    token = raw.strip()
    if not token or any(character.isspace() for character in token):
        raise InvocationRefused("token-file-invalid")
    return token


def prepare_output_dir(raw: str, fixture: Path) -> Path:
    output_dir = Path(raw)
    try:
        output_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    except OSError as error:
        raise InvocationRefused("output-dir-unusable") from error
    if output_dir.is_symlink():
        raise InvocationRefused("output-dir-symlink")
    if not output_dir.is_dir():
        raise InvocationRefused("output-dir-not-directory")
    if stat.S_IMODE(output_dir.stat().st_mode) & 0o077:
        raise InvocationRefused("output-dir-not-private")
    resolved_output = output_dir.resolve()
    resolved_fixture = fixture.resolve()
    if resolved_fixture == resolved_output or resolved_output in resolved_fixture.parents:
        raise InvocationRefused("output-dir-must-not-contain-fixture")
    return output_dir


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


@dataclass
class Response:
    status: int
    headers: object
    data: bytes


@dataclass
class RequestLogEntry:
    method: str
    path: str
    status: int


class Client:
    """Minimal bearer-authenticated JSON client for the processing surface."""

    def __init__(
        self,
        base_url: str,
        token: str,
        timeout: float = 30.0,
        max_json_bytes: int = MAX_JSON_BYTES,
        max_download_bytes: int = MAX_DOWNLOAD_BYTES,
    ):
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout
        self.max_json_bytes = max_json_bytes
        self.max_download_bytes = max_download_bytes
        self.log: list[RequestLogEntry] = []
        self._opener = urllib.request.build_opener(_NoRedirect)

    def request(
        self,
        method: str,
        path: str,
        payload: dict | None = None,
        max_bytes: int | None = None,
    ) -> Response:
        url = self.base_url + path
        body = None
        headers = {
            "Authorization": f"Bearer {self.token}",
            "slipstream-cli-contract": "1",
            "Accept": "application/json",
        }
        if payload is not None:
            body = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=body, headers=headers, method=method)
        limit = max_bytes if max_bytes is not None else self.max_json_bytes
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                status = response.status
                response_headers = response.headers
                data = self._read_bounded(response, limit)
        except urllib.error.HTTPError as error:
            with error:
                data = self._read_bounded(error, limit)
                response_headers = error.headers
            status = error.code
        except (urllib.error.URLError, OSError, TimeoutError) as error:
            raise TransportFailure("transport-unreachable", {"error": type(error).__name__})
        self.log.append(RequestLogEntry(method, path, status))
        return Response(status, response_headers, data)

    @staticmethod
    def _read_bounded(stream, limit: int) -> bytes:
        chunks = []
        remaining = limit + 1
        while remaining > 0:
            chunk = stream.read(min(remaining, 1024 * 1024))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
        if len(data) > limit:
            raise TransportFailure("response-too-large", {"limit": limit})
        return data

    def request_json(self, method: str, path: str, payload: dict | None = None) -> tuple[Response, object]:
        response = self.request(method, path, payload=payload)
        try:
            parsed = json.loads(response.data)
        except (UnicodeDecodeError, json.JSONDecodeError):
            # `require_success` classifies a non-JSON body together with the
            # HTTP status; a non-JSON success is refused there.
            parsed = None
        return response, parsed


def structured_code(payload: object) -> str | None:
    """The authoritative error code of a contract refusal, if any.

    The merged server maps every refusal onto the shared error envelope
    (`cli_error`): `{"error": {"code": ..., "message": ..., "details": ...}}`.
    A flat top-level `code` is accepted as well, so both conforming shapes
    and the spec's flat wording are honored.
    """
    if isinstance(payload, dict):
        code = payload.get("code")
        if isinstance(code, str) and code:
            return code
        error = payload.get("error")
        if isinstance(error, dict):
            code = error.get("code")
            if isinstance(code, str) and code:
                return code
    return None


def require_success(response: Response, parsed: object, context: str, accepted=(200,)):
    """Fail any non-accepted response, including required undeployed routes."""
    if response.status in accepted:
        if not isinstance(parsed, dict):
            raise AcceptanceFailure(f"{context}-payload-not-object", {})
        return
    code = structured_code(parsed)
    if response.status == 404 and code is None:
        raise AcceptanceFailure(
            "route-not-deployed",
            {"path": context, "status": response.status},
        )
    raise AcceptanceFailure(
        f"{context}-refused",
        {"status": response.status, "code": code},
    )


@dataclass
class StepRecord:
    name: str
    status: str = "pass"
    reason: str | None = None
    detail: dict = field(default_factory=dict)
    startedAt: str = ""
    finishedAt: str = ""
    durationSeconds: float = 0.0
    requests: list = field(default_factory=list)

    def as_dict(self) -> dict:
        value = {
            "name": self.name,
            "status": self.status,
            "startedAt": self.startedAt,
            "finishedAt": self.finishedAt,
            "durationSeconds": round(self.durationSeconds, 3),
            "requests": self.requests,
        }
        if self.reason is not None:
            value["reason"] = self.reason
        if self.detail:
            value["detail"] = self.detail
        return value


class Runner:
    def __init__(self, *, base_url, token, fixture, output_dir,
                 request_timeout=30.0, settlement_timeout=900.0,
                 preview_timeout=120.0, poll_interval=2.0,
                 max_download_bytes=MAX_DOWNLOAD_BYTES,
                 accepted_profile_digests=PINNED_SOURCE_PROFILE_DIGESTS,
                 expected_identities=None, monotonic=time.monotonic):
        self.client = Client(base_url, token, request_timeout, max_download_bytes=max_download_bytes)
        self.fixture, self.output_dir = fixture, output_dir
        self.settlement_timeout, self.preview_timeout = settlement_timeout, preview_timeout
        self.poll_interval, self.monotonic = poll_interval, monotonic
        self.accepted_profile_digests = accepted_profile_digests
        self.expected_identities = expected_identities or {}
        self.steps, self.identities, self.written_files = [], {}, []
        self.invariance_before = {}
        self.photo_id = self.source_revision = self.recipe_version = None
        self.observed_recipe = self.saved_recipe = None
        self.module = {}
        self.step_id = new_request_identity("darktable")
        self.export_request = self.export_identity = self.export_artifact = None

    def _step(self, name, function, gates=()):
        record = StepRecord(name=name, startedAt=now_iso())
        start, mark = self.monotonic(), len(self.client.log)
        blocked = next((step for step in self.steps if step.name in gates and step.status != "pass"), None)
        if blocked:
            record.status, record.reason = "skipped", "prerequisite-not-passed"
            record.detail = {"prerequisite": blocked.name}
        else:
            try:
                record.detail = function() or {}
            except (AcceptanceFailure, TransportFailure) as error:
                record.status, record.reason, record.detail = "fail", error.reason, error.detail
            except OSError as error:
                record.status, record.reason = "fail", "local-file-unavailable"
                record.detail = {"error": type(error).__name__}
        record.durationSeconds = self.monotonic() - start
        record.finishedAt = now_iso()
        record.requests = [{"method": entry.method, "path": entry.path, "status": entry.status}
                           for entry in self.client.log[mark:]]
        self.steps.append(record)

    def _step_discovery(self):
        response, payload = self.client.request_json("GET", MODULES_PATH)
        require_success(response, payload, "module-discovery")
        modules = payload.get("modules")
        if not isinstance(modules, list):
            raise AcceptanceFailure("modules-not-array")
        peers = [module for module in modules if isinstance(module, dict) and module.get("id", {}).get("name") == "darktable"]
        if len(peers) != 1:
            raise AcceptanceFailure("darktable-discovery-ambiguous")
        self.module = peers[0]
        if self.module.get("parameterVersions") != ["darktable-params-1"] or self.module.get("id", {}).get("adapterVersion") != "darktable-adapter-1":
            raise AcceptanceFailure("darktable-qualification-version-changed")
        if not isinstance(self.module.get("parameterSchema"), dict):
            raise AcceptanceFailure("darktable-parameter-schema-missing")
        if self.module["parameterSchema"].get("default") != qualified_darktable_tree(0.0):
            raise AcceptanceFailure("darktable-qualified-default-changed")
        limits = self.module.get("limits", {})
        if any(not isinstance(limits.get(key), int) or isinstance(limits[key], bool) or limits[key] <= 0
               for key in ("maxInputBytes", "maxParameterBytes", "maxOutputPixels", "deadlineMillis")):
            raise AcceptanceFailure("darktable-limits-invalid")
        self.identities["module"] = "darktable"
        self.identities["adapterSchemaVersion"] = "darktable-adapter-1"
        self.identities["stepId"] = self.step_id
        return {"module": "darktable", "availability": self.module.get("availability"), "limits": limits}

    def _require_ready(self):
        if self.module.get("availability", {}).get("state") != "ready":
            raise AcceptanceFailure("darktable-not-ready", {"availability": self.module.get("availability")})

    def _read_recipe(self, context):
        deadline = self.monotonic() + self.settlement_timeout
        while True:
            response, payload = self.client.request_json("GET", RECIPE_PATH.format(id=self.photo_id))
            require_success(response, payload, context)
            if payload.get("sourceRevision") == "" and self.monotonic() < deadline:
                time.sleep(min(self.poll_interval, max(0, deadline - self.monotonic())))
                continue
            facts, problems = validate_recipe_read(payload, self.photo_id)
            if problems:
                raise AcceptanceFailure(context + "-invalid", {"problems": problems})
            return facts

    def _step_recipe_read(self):
        facts = self._read_recipe("recipe-read")
        self.source_revision = facts["sourceRevision"]
        self.observed_recipe = facts["recipe"]
        self.recipe_version = (facts["recipe"] or {}).get("revision")
        if self.observed_recipe:
            self.step_id = self.observed_recipe.get("currentStepId") or self.step_id
            selected = next((step for step in self.observed_recipe["steps"] if step["stepId"] == self.step_id), None)
            if selected is None or selected.get("module") != "darktable" or selected.get("input") != self._original_input():
                raise AcceptanceFailure("fixture-selected-step-not-qualified")
            if selected.get("parameters", {}).get("schemaVersion") != "darktable-params-1" or selected["parameters"].get("tree") not in (qualified_darktable_tree(0.0), qualified_darktable_tree(1.0)):
                raise AcceptanceFailure("fixture-retained-intent-outside-qualified-tree")
        self.identities.update(sourceRevision=self.source_revision, stepId=self.step_id)
        return {"hadSavedRecipe": self.observed_recipe is not None, "sourceRevision": self.source_revision}

    def _original_input(self):
        return {"kind": "original", "photoId": self.photo_id, "sourceRevision": self.source_revision}

    def _recipe_intent(self, exposure):
        steps = copy.deepcopy((self.observed_recipe or {}).get("steps", []))
        step = {"stepId": self.step_id, "module": "darktable", "input": self._original_input(),
                "parameters": {"schemaVersion": "darktable-params-1", "tree": qualified_darktable_tree(exposure)}}
        for index, prior in enumerate(steps):
            if prior["stepId"] == self.step_id:
                steps[index] = step
                break
        else:
            steps.append(step)
        return {"currentStepId": self.step_id, "steps": steps}

    def _guarded_save(self, intent, purpose):
        body = dict(copy.deepcopy(intent), requestId=new_request_identity(purpose),
                    expectedRecipeRevision=self.recipe_version, expectedSourceRevision=self.source_revision)
        response, payload = self.client.request_json("POST", RECIPE_PATH.format(id=self.photo_id), body)
        require_success(response, payload, "recipe-save", accepted=(200, 201))
        recipe = payload.get("recipe")
        if payload.get("outcome") not in ("saved", "unchanged") or not isinstance(recipe, dict):
            raise AcceptanceFailure("recipe-save-outcome-unexpected")
        expected = dict(intent, photoId=self.photo_id, sourceRevision=self.source_revision)
        problems = validate_captured_identity(recipe, expected)
        version = recipe.get("revision")
        if not isinstance(version, str) or not version or payload.get("recipeVersion") != version or payload.get("sourceRevision") != self.source_revision:
            problems.append("save-guards-mismatch")
        if problems:
            raise AcceptanceFailure("recipe-save-invalid", {"problems": problems})
        self.recipe_version, self.saved_recipe = version, recipe
        return {"revision": version, "outcome": payload["outcome"]}

    def _step_save_exposure(self):
        return self._guarded_save(self._recipe_intent(1.0), "save")

    def _step_save_reversal(self):
        baseline = ({key: copy.deepcopy(self.observed_recipe[key]) for key in ("currentStepId", "steps")}
                    if self.observed_recipe else {"currentStepId": None, "steps": []})
        result = self._guarded_save(baseline, "reversal")
        self._step_recipe_reopen()
        # Restore the qualified +1 EV selection for explicit processing.
        self._guarded_save(self._recipe_intent(1.0), "restore-selection")
        return result

    def _step_recipe_reopen(self):
        facts = self._read_recipe("recipe-reopen")
        if facts["recipe"] != self.saved_recipe or facts["sourceRevision"] != self.source_revision:
            raise AcceptanceFailure("recipe-reopen-changed")
        return {"revision": self.recipe_version, "unchanged": True}

    def _captured(self):
        selected = next(step for step in self.saved_recipe["steps"] if step["stepId"] == self.step_id)
        return {"photoId": self.photo_id, "requestId": self.export_request["requestId"],
                "stepId": self.step_id, "module": "darktable", "recipeRevision": self.recipe_version,
                "sourceRevision": self.source_revision, "parameters": selected["parameters"],
                "input": selected["input"], "adapterSchemaVersion": "darktable-adapter-1"}

    def _step_submit_export(self):
        self._require_ready()
        self.export_request = {"requestId": new_request_identity("export"), "stepId": self.step_id,
            "expectedRecipeRevision": self.recipe_version, "expectedSourceRevision": self.source_revision}
        response, payload = self.client.request_json("POST", EXPORTS_PATH.format(id=self.photo_id), self.export_request)
        require_success(response, payload, "export-submit", accepted=(202,))
        receipt = payload.get("receipt")
        problems = validate_captured_identity(receipt, self._captured())
        if payload.get("outcome") != "accepted":
            problems.append("outcome-not-accepted")
        if not isinstance(receipt, dict) or receipt.get("state") not in ("accepted", "executing", "succeeded"):
            problems.append("receipt-state-invalid")
        bundle = receipt.get("bundleId") if isinstance(receipt, dict) else None
        if not isinstance(bundle, str) or not _LOWER_HEX_64.fullmatch(bundle):
            problems.append("bundleId-invalid")
        if self.expected_identities.get("bundleSha256") and bundle != self.expected_identities["bundleSha256"]:
            problems.append("bundleId-mismatch")
        if problems:
            raise AcceptanceFailure("export-submit-invalid", {"problems": problems})
        self.export_identity = dict(self._captured(), bundleId=bundle)
        self.identities.update(bundleId=bundle, exportRequestId=self.export_request["requestId"])
        return {"requestId": self.export_request["requestId"], "state": receipt["state"]}

    def _step_export_settlement(self):
        deadline = self.monotonic() + self.settlement_timeout
        path = EXPORTS_PATH.format(id=self.photo_id) + "/" + self.export_request["requestId"]
        while True:
            response, work = self.client.request_json("GET", path)
            require_success(response, work, "export-status")
            problems = validate_captured_identity(work, self.export_identity)
            if work.get("state") not in ("accepted", "executing", "succeeded", "failed", "cancelled"):
                problems.append("state-invalid")
            if problems:
                raise AcceptanceFailure("export-status-invalid", {"problems": problems})
            if work["state"] in ("succeeded", "failed", "cancelled"):
                break
            if self.monotonic() >= deadline:
                raise AcceptanceFailure("export-settlement-timeout", {"state": work["state"]})
            time.sleep(min(self.poll_interval, max(0, deadline - self.monotonic())))
        if work["state"] != "succeeded":
            raise AcceptanceFailure("export-not-succeeded", {"state": work["state"], "failureReason": work.get("failureReason")})
        terminal_at, retain_until = work.get("terminalAt"), work.get("retainUntil")
        if (not isinstance(terminal_at, int) or isinstance(terminal_at, bool)
                or not isinstance(retain_until, int) or isinstance(retain_until, bool)
                or retain_until <= terminal_at or retain_until <= time.time()
                or work.get("failureReason") is not None):
            raise AcceptanceFailure("export-terminal-receipt-invalid")
        artifact_id = work.get("artifactId")
        if not valid_request_identity(artifact_id):
            raise AcceptanceFailure("artifact-id-invalid")
        response, artifact = self.client.request_json("GET", ARTIFACT_PATH.format(id=artifact_id))
        require_success(response, artifact, "artifact-inspect")
        expected = {key: value for key, value in self.export_identity.items() if key not in ("requestId", "recipeRevision", "sourceRevision", "input")}
        expected["artifactId"] = artifact_id
        problems = validate_captured_identity(artifact, expected)
        input_evidence = artifact.get("input", {})
        snapshot = self.invariance_before[str(self.fixture)]
        if input_evidence != {"binding": self._original_input(), "sha256": snapshot["sha256"], "byteLength": snapshot["byteLength"]}:
            problems.append("artifact-input-evidence-mismatch")
        contract = artifact.get("outputContract", {})
        for key, value in {"format": "tiff", "precision": "float32", "colorSpace": "prophoto-rgb", "transfer": "linear", "encoding": "deflate"}.items():
            if contract.get(key) != value:
                problems.append("artifact-output-" + key + "-invalid")
        geometry = contract.get("geometry", {})
        if any(not isinstance(geometry.get(key), int) or isinstance(geometry[key], bool) or geometry[key] <= 0 for key in ("width", "height")):
            problems.append("artifact-geometry-invalid")
        _, size_problems = artifact_download_limit(artifact.get("byteLength"), self.client.max_download_bytes)
        problems.extend(size_problems)
        if not _LOWER_HEX_64.fullmatch(str(artifact.get("sha256", ""))):
            problems.append("artifact-sha256-invalid")
        published = parse_timestamp(artifact.get("publishedAt"))
        expires = parse_timestamp(artifact.get("expiresAt"))
        if published is None or expires is None or expires <= published or expires <= datetime.now(timezone.utc):
            problems.append("artifact-retention-invalid")
        if artifact.get("filename") != f"{artifact_id}.tif":
            problems.append("artifact-filename-invalid")
        if problems:
            raise AcceptanceFailure("artifact-inspect-invalid", {"problems": problems})
        self.export_artifact = artifact
        self.identities["exportState"] = work["state"]
        return {"state": work["state"], "artifact": artifact}

    def _step_download_artifact(self):
        artifact = self.export_artifact
        artifact_id = artifact["artifactId"]
        destination = self.output_dir / artifact["filename"]
        if destination.is_symlink() or destination.exists() or self.output_dir.resolve() not in destination.resolve().parents:
            raise AcceptanceFailure("artifact-path-occupied-or-unsafe")
        limit, _ = artifact_download_limit(artifact["byteLength"], self.client.max_download_bytes)
        response = self.client.request("GET", ARTIFACT_PATH.format(id=artifact_id) + "/bytes", max_bytes=limit)
        if response.status != 200:
            raise AcceptanceFailure("artifact-download-refused", {"status": response.status})
        problems = []
        geometry = artifact["outputContract"]["geometry"]
        headers = {"slipstream-artifact-id": artifact_id, "slipstream-artifact-photo-id": self.photo_id,
            "slipstream-artifact-step-id": self.step_id, "slipstream-artifact-module": "darktable",
            "slipstream-artifact-adapter-schema-version": artifact["adapterSchemaVersion"],
            "slipstream-artifact-bundle-id": artifact["bundleId"], "slipstream-artifact-sha256": artifact["sha256"],
            "slipstream-artifact-byte-length": str(artifact["byteLength"]),
            "slipstream-artifact-width": str(geometry["width"]), "slipstream-artifact-height": str(geometry["height"]),
            "slipstream-artifact-filename": artifact["filename"],
            "slipstream-artifact-published-at": artifact["publishedAt"],
            "slipstream-artifact-expires-at": artifact["expiresAt"],
            "Content-Type": "image/tiff", "Content-Length": str(artifact["byteLength"]),
            "Content-Disposition": f'attachment; filename="{artifact["filename"]}"'}
        for header, expected in headers.items():
            if response.headers.get_all(header) != [expected]:
                problems.append(header + "-mismatch")
        if len(response.data) != artifact["byteLength"] or sha256_hex(response.data) != artifact["sha256"]:
            problems.append("artifact-bytes-mismatch")
        facts, tiff_problems = validate_development_tiff(response.data, geometry["width"], geometry["height"], self.accepted_profile_digests)
        problems.extend(tiff_problems)
        if problems:
            raise AcceptanceFailure("artifact-invalid", {"problems": problems, "tiff": facts})
        with destination.open("xb") as stream:
            os.chmod(destination, 0o600)
            stream.write(response.data)
        self.written_files.append(str(destination))
        self.identities["artifact"] = artifact
        return {"sha256": artifact["sha256"], "byteLength": len(response.data), "tiff": facts}

    def _step_preview(self):
        self._require_ready()
        path = PREVIEW_PATH.format(id=self.photo_id, step=urllib.parse.quote(self.step_id, safe=""))
        deadline = self.monotonic() + self.preview_timeout
        original_timeout = self.client.timeout
        self.client.timeout = min(original_timeout, self.preview_timeout)
        try:
            while True:
                response = self.client.request("GET", path, max_bytes=min(self.client.max_download_bytes, 16 * 1024 * 1024))
                if response.status != 202:
                    break
                if self.monotonic() >= deadline:
                    raise AcceptanceFailure("preview-render-timeout")
                time.sleep(min(self.poll_interval, max(0, deadline - self.monotonic())))
                self.client.timeout = min(original_timeout, max(0.001, deadline - self.monotonic()))
        finally:
            self.client.timeout = original_timeout
        if response.status != 200:
            raise AcceptanceFailure("preview-refused", {"status": response.status})
        prefix = "slipstream-processing-preview-"
        expected = {"photo-id": self.photo_id, "step-id": self.step_id, "module": "darktable",
            "source-revision": self.source_revision.encode("utf-8").hex(),
            "recipe-revision": self.recipe_version, "adapter-schema-version": "darktable-adapter-1",
            "bundle-id": self.identities["bundleId"], "sha256": sha256_hex(response.data)}
        problems = [key + "-mismatch" for key, value in expected.items() if response.headers.get_all(prefix + key) != [value]]
        parameters = next(step["parameters"] for step in self.saved_recipe["steps"] if step["stepId"] == self.step_id)
        parameter_digest = sha256_hex(json.dumps({"kind": "processing-parameters-v1",
            "schema_version": parameters["schemaVersion"], "parameters": parameters["tree"]},
            sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))
        if response.headers.get_all(prefix + "parameter-digest") != [parameter_digest]:
            problems.append("parameter-digest-mismatch")
        for key in ("parameter-digest", "output-contract", "identity"):
            if not _LOWER_HEX_64.fullmatch(response.headers.get(prefix + key, "")):
                problems.append(key + "-invalid")
        if response.headers.get("Content-Type") != "image/png":
            problems.append("content-type-invalid")
        bound = response.headers.get(prefix + "geometry", "")
        if not bound.isdigit() or not 0 < int(bound) <= 1224:
            problems.append("preview-geometry-invalid")
            bound = "1224"
        if not response.headers.get(prefix + "display-conversion"):
            problems.append("display-conversion-missing")
        facts, png_problems = validate_png(response.data, int(bound))
        problems.extend(png_problems)
        for key in ("width", "height"):
            if response.headers.get(prefix + key) != str(facts.get(key)):
                problems.append(key + "-mismatch")
        if problems:
            raise AcceptanceFailure("preview-invalid", {"problems": problems})
        return dict(facts, sha256=sha256_hex(response.data), geometryBound=int(bound))

    def _list_page_maximum(self) -> int:
        """The server-published Photo list page bound for query paging."""
        try:
            response, payload = self.client.request_json("GET", CAPABILITIES_PATH)
        except TransportFailure:
            return FALLBACK_LIST_PAGE_MAXIMUM
        if response.status != 200 or not isinstance(payload, dict):
            return FALLBACK_LIST_PAGE_MAXIMUM
        limits = payload.get("limits")
        maximum = limits.get("listPageMaximum") if isinstance(limits, dict) else None
        if (
            isinstance(maximum, int)
            and not isinstance(maximum, bool)
            and 1 <= maximum <= MAXIMUM_QUERY_LIMIT_BOUND
        ):
            return maximum
        return FALLBACK_LIST_PAGE_MAXIMUM

    def _step_resolve_photo(self) -> dict:
        page_limit = self._list_page_maximum()
        matches = []
        seen = 0
        cursor = None
        for _page in range(MAX_QUERY_PAGES):
            if cursor is None:
                response, payload = self.client.request_json(
                    "POST",
                    PHOTO_QUERIES_PATH,
                    payload={
                        "source": {"kind": "all"},
                        "kind": "raw",
                        "available": True,
                        "limit": page_limit,
                    },
                )
                require_success(response, payload, "photo-query")
            else:
                response, payload = self.client.request_json(
                    "GET", f"{PHOTO_QUERIES_PATH}/{cursor}"
                )
                require_success(response, payload, "photo-query-page")
            items = payload.get("items")
            if not isinstance(items, list):
                raise AcceptanceFailure("photo-query-items-invalid", {})
            seen += len(items)
            for item in items:
                if not isinstance(item, dict):
                    continue
                if item.get("state") == "missing":
                    continue
                if item.get("filename") == self.fixture.name and item.get(
                    "originalKind"
                ) == "raw" and item.get("originalAvailable") is True:
                    matches.append(item)
            cursor = payload.get("nextCursor")
            if not isinstance(cursor, str) or not cursor:
                break
        else:
            raise AcceptanceFailure("photo-query-pagination-unbounded", {"itemsSeen": seen})
        if not matches:
            raise AcceptanceFailure(
                "fixture-photo-not-found",
                {"filename": self.fixture.name, "rawPhotosSeen": seen},
            )
        if len(matches) > 1:
            raise AcceptanceFailure(
                "fixture-photo-ambiguous",
                {"filename": self.fixture.name, "matches": len(matches)},
            )
        self.photo_id = matches[0].get("id")
        if not isinstance(self.photo_id, str) or not self.photo_id:
            raise AcceptanceFailure("fixture-photo-id-invalid", {})
        self.identities["photoId"] = self.photo_id
        return {"photoId": self.photo_id, "photosSeen": seen, "listPageMaximum": page_limit}

    def _step_invariance_after(self) -> dict:
        snapshots, unreadable = self._current_snapshots()
        if unreadable:
            # A file we must prove unchanged cannot be read any more.  The
            # comparison cannot run, so the step fails and the report records
            # it instead of raising a traceback past the report.
            raise AcceptanceFailure(
                "invariance-snapshot-failed", {"unreadable": unreadable}
            )
        changes = invariance_changes(self.invariance_before, snapshots)
        if changes:
            raise AcceptanceFailure("original-mutated", {"changes": changes})
        return {"checked": sorted(snapshots), "unchanged": True}

    def _current_snapshots(self) -> tuple[dict, list]:
        snapshots = {}
        unreadable = []
        candidates = [self.fixture] + [
            path for path in external_xmp_sidecars(self.fixture) if path.exists()
        ]
        for path in candidates:
            try:
                snapshots[str(path)] = snapshot_original(path)
            except OSError as error:
                unreadable.append({"path": str(path), "error": type(error).__name__})
        return snapshots, unreadable

    def _invariance_before(self) -> None:
        if not self.fixture.is_file():
            raise InvocationRefused("fixture-not-regular-file")
        try:
            snapshots, unreadable = self._current_snapshots()
        except OSError as error:
            raise InvocationRefused("fixture-unreadable") from error
        if not snapshots or unreadable:
            raise InvocationRefused("fixture-unreadable")
        self.invariance_before = snapshots

    def run(self):
        start = self.monotonic()
        self._invariance_before()
        self._step("module-discovery", self._step_discovery)
        self._step("resolve-photo", self._step_resolve_photo, ("module-discovery",))
        self._step("read-recipe", self._step_recipe_read, ("resolve-photo",))
        self._step("save-exposure", self._step_save_exposure, ("read-recipe",))
        self._step("save-reversal", self._step_save_reversal, ("save-exposure",))
        self._step("submit-export", self._step_submit_export, ("save-reversal",))
        self._step("export-settlement", self._step_export_settlement, ("submit-export",))
        self._step("download-artifact", self._step_download_artifact, ("export-settlement",))
        self._step("recipe-reopen", self._step_recipe_reopen, ("save-reversal",))
        self._step("selected-preview", self._step_preview, ("submit-export", "recipe-reopen"))
        self._step("original-invariance", self._step_invariance_after)
        failed = [step for step in self.steps if step.status == "fail"]
        skipped = [step for step in self.steps if step.status == "skipped"]
        return {"scope": "selected-darktable-step-acceptance", "issue": 496,
            "status": "failed" if failed else "blocked" if skipped else "passed",
            "startedAt": self.steps[0].startedAt, "finishedAt": now_iso(),
            "durationSeconds": round(self.monotonic() - start, 3),
            "target": {"baseUrl": self.client.base_url},
            "operatorSuppliedIdentities": self.expected_identities, "identities": self.identities,
            "fixture": self.invariance_before.get(str(self.fixture), {}),
            "sidecars": [value for path, value in self.invariance_before.items() if path != str(self.fixture)],
            "steps": [step.as_dict() for step in self.steps],
            "notRun": [{"step": step.name, "reason": step.reason} for step in skipped],
            "writtenFiles": self.written_files,
            "qualification": {"module": "darktable", "exposureEv": [0, 1],
                "parameterSchemaVersion": "darktable-params-1", "fixtureOnly": True,
                "standaloneSpektraFilm": "not-covered; requires separately admitted fixture and resources"}}


def render_summary(report):
    return "\n".join([f"[{step['status'].upper()}] {step['name']}" +
        (f" ({step['reason']})" if step.get("reason") else "") for step in report["steps"]] +
        [f"Status: {report['status']}", "Standalone SpektraFilm qualification not covered."])


def download_bound(value: str) -> int:
    """Parse `--max-download-bytes`: a positive read bound within the hard maximum."""
    try:
        parsed = int(value, 10)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a decimal byte count") from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive byte count")
    if parsed > MAXIMUM_DOWNLOAD_BYTES:
        raise argparse.ArgumentTypeError(
            f"must not exceed the service's hard output maximum of {MAXIMUM_DOWNLOAD_BYTES} bytes"
        )
    return parsed


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base-url", required=True, help="Deployment base URL (HTTP or HTTPS; HTTP prints an unencrypted-connection warning).")
    parser.add_argument("--token-file", required=True, type=Path, help="File holding the bearer token.")
    parser.add_argument("--fixture", required=True, type=Path, help="Approved-profile RAW fixture path.")
    parser.add_argument("--output-dir", required=True, help="Private directory for downloaded artifacts.")
    parser.add_argument("--i-acknowledge-this-is-an-acceptance-instance", action="store_true")
    parser.add_argument(
        "--expected-bundle-sha256",
        help="Bundle digest captured by the selected Export (the image build's printed bundle identity).",
    )
    parser.add_argument("--request-timeout", type=float, default=30.0)
    parser.add_argument("--settlement-timeout", type=float, default=900.0)
    parser.add_argument("--preview-timeout", type=float, default=120.0)
    parser.add_argument("--poll-interval", type=float, default=2.0)
    parser.add_argument(
        "--max-download-bytes",
        type=download_bound,
        default=MAX_DOWNLOAD_BYTES,
        help=(
            "Read bound for artifact downloads: the deployment's retained-output allowance in bytes "
            "(`SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES`, docs/deployment.md), which must cover a "
            f"full-resolution Development TIFF. Defaults to {MAX_DOWNLOAD_BYTES}; the service's hard "
            f"output maximum is {MAXIMUM_DOWNLOAD_BYTES}."
        ),
    )
    return parser.parse_args(argv)


def refused_report(reason: str) -> dict:
    return {
        "scope": "photo-development-acceptance-workflow",
        "issue": 496,
        "status": "refused",
        "reason": reason,
    }


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(argv)
    if any(not math.isfinite(value) or value <= 0 or value > 3600 for value in
           (arguments.request_timeout, arguments.settlement_timeout, arguments.preview_timeout, arguments.poll_interval)):
        print(json.dumps(refused_report("timeout-outside-finite-bound")))
        return 2
    if not arguments.i_acknowledge_this_is_an_acceptance_instance:
        print(
            "Refusing to run: pass --i-acknowledge-this-is-an-acceptance-instance to confirm "
            "the target is a dedicated acceptance deployment, never the operator's live library.",
            file=sys.stderr,
        )
        print(json.dumps(refused_report("acceptance-instance-not-acknowledged"), indent=2, sort_keys=True))
        return 2
    try:
        base_url = normalize_base_url(arguments.base_url)
        if base_url.startswith("http:"):
            print("Warning: HTTP is unencrypted; photos and credentials may be observed in transit.", file=sys.stderr)
        token = read_token_file(arguments.token_file)
        fixture = arguments.fixture
        if not fixture.is_file():
            raise InvocationRefused("fixture-not-regular-file")
        output_dir = prepare_output_dir(arguments.output_dir, fixture)
    except InvocationRefused as error:
        print(f"Refusing to run: {error.reason}.", file=sys.stderr)
        print(json.dumps(refused_report(error.reason), indent=2, sort_keys=True))
        return 2
    expected = {
        key: value
        for key, value in {
            "bundleSha256": arguments.expected_bundle_sha256,
        }.items()
        if value
    }
    runner = Runner(
        base_url=base_url,
        token=token,
        fixture=fixture,
        output_dir=output_dir,
        request_timeout=arguments.request_timeout,
        settlement_timeout=arguments.settlement_timeout,
        preview_timeout=arguments.preview_timeout,
        poll_interval=arguments.poll_interval,
        max_download_bytes=arguments.max_download_bytes,
        expected_identities=expected,
    )
    try:
        report = runner.run()
    except InvocationRefused as error:
        print(f"Refusing to run: {error.reason}.", file=sys.stderr)
        print(json.dumps(refused_report(error.reason), indent=2, sort_keys=True))
        return 2
    print(json.dumps(report, indent=2, sort_keys=True))
    print(render_summary(report), file=sys.stderr)
    if report["status"] == "passed":
        return 0
    if report["status"] == "failed":
        return 1
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
