"""Photo development workflow acceptance runner for Issue #334.

Drives a deployed Slipstream instance through the real HTTP surface defined by
`design/photo-development.md` (Service Surface and Wire contract): capability
read, Photo resolution, Edit Recipe read, guarded exposure save and reversal,
Edit Preview, Development TIFF Export through terminal settlement, and artifact
download with byte-level validation.  The tool never writes inside the
instance's Library or Originals directory: the only files it creates are
downloaded artifacts inside the explicit output directory, and it proves the
fixture Original and any external XMP sidecar unchanged by hashing them before
and after the run without opening them for writing.

The Film (`finished-jpeg`) stage is owned by Issue #332 and is not implemented
yet; the runner reports that stage as not covered instead of pretending to
exercise it.

The tool must never run against an operator's live library.  It refuses to run
without `--i-acknowledge-this-is-an-acceptance-instance`, and the documented
target is a dedicated acceptance deployment (see tools/processing/README.md).

Exit codes: 0 when every step that ran passed, 1 when any step failed, and 2
when the run was blocked (steps could not run, for example because a route of
the merged wire contract is not deployed yet) or the invocation was refused.
A JSON report is printed on standard output; a human-readable summary goes to
standard error.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import secrets
import stat
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

MAX_JSON_BYTES = 1024 * 1024
MAX_DOWNLOAD_BYTES = 512 * 1024 * 1024
DOWNLOAD_SLACK_BYTES = 65536
MAX_TOKEN_BYTES = 4096
MAX_QUERY_PAGES = 50

CAPABILITY_PATH = "/api/processing/capability"
PHOTO_QUERIES_PATH = "/api/photo-queries"
EDIT_RECIPE_PATH = "/api/photos/{id}/edit-recipe"
EDIT_PREVIEW_PATH = "/api/photos/{id}/edit-preview/{stage}"
PHOTO_EXPORTS_PATH = "/api/photos/{id}/exports"
EXPORT_PATH = "/api/exports/{id}"
EXPORT_ARTIFACT_PATH = "/api/exports/{id}/artifact"

DEVELOPMENT_TARGET = "development-tiff"
DEVELOP_STAGE = "develop"
DISPLAY_TRANSFORM_IDENTITY = "display-transform-v1"
DEVELOPMENT_CONTENT_TYPE = "image/tiff"

# The pinned Development TIFF source profiles from design/development-color.md:
# the bundle asset and the legacy-normalized profile the qualified darktable run
# embeds.  They differ only in description-tag bytes.
PINNED_SOURCE_PROFILE_DIGESTS = (
    "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed",
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe",
)

CAPABILITY_STATES = (
    "disabled",
    "launcher-unavailable",
    "bundle-unavailable",
    "source-unsupported",
    "resource-unavailable",
    "ready",
)
STAGE_STATES = ("ready", "unavailable", "unsupported")
SOURCE_SUPPORT_STATES = ("supported", "unavailable", "unsupported")

ARTIFACT_METADATA_FIELDS = (
    "exportId",
    "target",
    "stage",
    "contentType",
    "width",
    "height",
    "profileIdentity",
    "byteLength",
    "sha256",
    "expiresAt",
)
PREVIEW_METADATA_FIELDS = (
    "photoId",
    "stage",
    "contentType",
    "width",
    "height",
    "byteLength",
    "sha256",
    "sourceRevision",
    "recipeVersion",
    "displayTransform",
    "expiresAt",
)

_SAVE_OUTCOMES = (
    "saved",
    "unchanged",
    "unknown",
    "receipt_expired",
    "recipe_conflict",
    "source_changed",
    "requires_rebind",
    "request_conflict",
    "missing_recipe",
    "unsupported",
    "invalid_settings",
    "unavailable",
)
_EXPORT_STATES = ("queued", "running", "succeeded", "failed", "cancelled")
_LOWER_HEX_64 = re.compile(r"\A[0-9a-f]{64}\Z")
_REQUEST_IDENTITY = re.compile(r"\A[A-Za-z0-9._-]{1,128}\Z")
_LOOPBACK_HOSTS = {"127.0.0.1", "::1", "localhost"}


class AcceptanceFailure(Exception):
    """A step failed for the recorded reason."""

    def __init__(self, reason: str, detail: dict | None = None):
        super().__init__(reason)
        self.reason = reason
        self.detail = detail or {}


class RouteMissing(Exception):
    """A route of the merged wire contract is not deployed; the step is skipped."""

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


def validate_capability(payload: object) -> tuple[dict, list]:
    """Validate `GET /api/processing/capability` against the wire contract."""
    problems: list = []
    facts: dict = {}
    if not isinstance(payload, dict):
        return facts, ["payload-not-object"]
    facts["state"] = payload.get("state")
    if payload.get("state") not in CAPABILITY_STATES:
        problems.append("state-outside-closed-set")
    facts["bundleId"] = payload.get("bundleId")
    if not isinstance(payload.get("bundleId"), str) or not payload.get("bundleId"):
        problems.append("bundleId-missing-or-not-string")
    facts["incarnation"] = payload.get("incarnation")
    if not isinstance(payload.get("incarnation"), str) or not payload.get("incarnation"):
        problems.append("incarnation-missing-or-not-string")
    exposure = payload.get("exposure")
    if not isinstance(exposure, dict):
        problems.append("exposure-missing-or-not-object")
    else:
        for key in ("minimumEv", "maximumEv", "stepEv"):
            value = exposure.get(key)
            if not isinstance(value, (int, float)) or isinstance(value, bool):
                problems.append(f"exposure-{key}-missing-or-not-number")
        facts["exposure"] = exposure
    profiles = payload.get("profiles")
    if not isinstance(profiles, list):
        problems.append("profiles-missing-or-not-array")
        facts["profiles"] = []
    else:
        cleaned = []
        for index, profile in enumerate(profiles):
            if not isinstance(profile, dict):
                problems.append(f"profiles-{index}-not-object")
                continue
            entry = {
                "profileId": profile.get("profileId"),
                "whiteBalanceModes": profile.get("whiteBalanceModes"),
                "whiteBalanceRanges": profile.get("whiteBalanceRanges"),
            }
            if not isinstance(entry["profileId"], str) or not entry["profileId"]:
                problems.append(f"profiles-{index}-profileId-missing")
            if not isinstance(entry["whiteBalanceModes"], list):
                problems.append(f"profiles-{index}-whiteBalanceModes-missing")
            if entry["whiteBalanceRanges"] is not None and not isinstance(
                entry["whiteBalanceRanges"], dict
            ):
                problems.append(f"profiles-{index}-whiteBalanceRanges-invalid")
            cleaned.append(entry)
        facts["profiles"] = cleaned
    stages = payload.get("stages")
    if not isinstance(stages, dict) or set(stages) != {"develop", "film"}:
        problems.append("stages-must-name-develop-and-film")
        facts["stages"] = {}
    else:
        for key, value in stages.items():
            if value not in STAGE_STATES:
                problems.append(f"stages-{key}-outside-closed-set")
        facts["stages"] = dict(stages)
    return facts, problems


def validate_recipe_read(payload: object, photo_id: str) -> tuple[dict, list]:
    """Validate `GET /api/photos/{id}/edit-recipe` against the wire contract."""
    problems: list = []
    facts: dict = {}
    if not isinstance(payload, dict):
        return facts, ["payload-not-object"]
    if payload.get("photoId") != photo_id:
        problems.append("photoId-mismatch")
    source_revision = payload.get("sourceRevision")
    facts["sourceRevision"] = source_revision
    if source_revision is not None and (
        not isinstance(source_revision, str) or not source_revision
    ):
        problems.append("sourceRevision-not-string-or-null")
    support = payload.get("sourceSupport")
    facts["sourceSupport"] = support
    if support not in SOURCE_SUPPORT_STATES:
        problems.append("sourceSupport-outside-closed-set")
    if support == "unavailable":
        if source_revision is not None:
            problems.append("sourceRevision-must-be-null-when-unavailable")
        if payload.get("supportReason") not in ("original-missing", "original-unreadable"):
            problems.append("supportReason-invalid-for-unavailable")
    else:
        if payload.get("supportReason") is not None:
            problems.append("supportReason-must-be-null-unless-unavailable")
    recipe = payload.get("recipe")
    facts["recipe"] = recipe
    if recipe is not None:
        if not isinstance(recipe, dict):
            problems.append("recipe-not-object-or-null")
        else:
            if not isinstance(recipe.get("recipeVersion"), str) or not recipe.get("recipeVersion"):
                problems.append("recipe-recipeVersion-missing")
            exposure = recipe.get("exposureEv")
            if not isinstance(exposure, (int, float)) or isinstance(exposure, bool):
                problems.append("recipe-exposureEv-missing-or-not-number")
            white_balance = recipe.get("whiteBalance")
            if not isinstance(white_balance, dict) or not isinstance(
                white_balance.get("mode"), str
            ):
                problems.append("recipe-whiteBalance-invalid")
    if not isinstance(payload.get("processingAvailable"), bool):
        problems.append("processingAvailable-missing-or-not-boolean")
    controls = payload.get("controls")
    if not isinstance(controls, dict):
        problems.append("controls-missing-or-not-object")
    else:
        exposure_controls = controls.get("exposure")
        if not isinstance(exposure_controls, dict):
            problems.append("controls-exposure-missing")
        else:
            for key in ("minimumEv", "maximumEv", "stepEv"):
                if not isinstance(exposure_controls.get(key), (int, float)) or isinstance(
                    exposure_controls.get(key), bool
                ):
                    problems.append(f"controls-exposure-{key}-invalid")
        if not isinstance(controls.get("whiteBalanceModes"), list):
            problems.append("controls-whiteBalanceModes-missing")
        facts["controls"] = controls
    return facts, problems


def build_save_body(
    request_id: str,
    expected_recipe_version: str | None,
    expected_source_revision: str,
    exposure_ev: float,
    white_balance_mode: str = "as-shot",
) -> dict:
    return {
        "requestId": request_id,
        "expectedRecipeVersion": expected_recipe_version,
        "expectedSourceRevision": expected_source_revision,
        "settings": {
            "exposureEv": exposure_ev,
            "whiteBalance": {"mode": white_balance_mode},
        },
    }


def validate_save_response(payload: object) -> tuple[dict, list]:
    """Validate a guarded-save response for the outcome the caller asserts."""
    problems: list = []
    facts: dict = {}
    if not isinstance(payload, dict):
        return facts, ["payload-not-object"]
    outcome = payload.get("outcome")
    facts["outcome"] = outcome
    if outcome not in _SAVE_OUTCOMES:
        problems.append("outcome-outside-closed-set")
        return facts, problems
    if outcome in ("saved", "unchanged"):
        version = payload.get("recipeVersion")
        if not isinstance(version, str) or not version:
            problems.append("recipeVersion-missing-for-committed-outcome")
        facts["recipeVersion"] = version
        revision = payload.get("sourceRevision")
        if not isinstance(revision, str) or not revision:
            problems.append("sourceRevision-missing-for-committed-outcome")
        facts["sourceRevision"] = revision
    if outcome in (
        "recipe_conflict",
        "source_changed",
        "requires_rebind",
        "request_conflict",
    ):
        facts["currentSourceRevision"] = payload.get("currentSourceRevision")
        facts["currentRecipeVersion"] = payload.get("currentRecipeVersion")
    return facts, problems


def choose_exposure(controls: dict, baseline: float | None) -> float:
    """Pick one guard-safe exposure step away from the baseline, on the grid."""
    exposure = controls.get("exposure", {})
    minimum = exposure.get("minimumEv")
    maximum = exposure.get("maximumEv")
    step = exposure.get("stepEv")
    if step is None or step <= 0:
        raise AcceptanceFailure("exposure-step-invalid", {"stepEv": step})
    base = 0.0 if baseline is None else baseline
    steps = round((base - minimum) / step)
    snapped = round(minimum + steps * step, 9)
    up = round(snapped + step, 9)
    down = round(snapped - step, 9)
    if up <= maximum:
        return up
    if down >= minimum:
        return down
    raise AcceptanceFailure(
        "no-exposure-headroom",
        {"minimumEv": minimum, "maximumEv": maximum, "stepEv": step},
    )


def exposure_on_grid(controls: dict, value: float) -> float:
    """Snap a target exposure onto the approved grid and clamp into range."""
    exposure = controls.get("exposure", {})
    minimum = exposure.get("minimumEv")
    maximum = exposure.get("maximumEv")
    step = exposure.get("stepEv")
    if step is None or step <= 0:
        raise AcceptanceFailure("exposure-step-invalid", {"stepEv": step})
    base = 0.0 if value is None else value
    steps = round((base - minimum) / step)
    snapped = round(minimum + steps * step, 9)
    return min(maximum, max(minimum, snapped))


def collect_metadata_headers(headers, fields) -> tuple[dict, list]:
    """Collect the closed typed metadata set from response headers."""
    metadata: dict = {}
    problems: list = []
    for name in fields:
        values = headers.get_all(name)
        if not values:
            problems.append(f"header-{name}-missing")
            continue
        if len(values) != 1:
            problems.append(f"header-{name}-repeated")
            continue
        metadata[name] = values[0]
    return metadata, problems


def header_object_mismatches(metadata: dict, artifact: dict) -> list:
    """Compare download headers with the inspect artifact object, field for field."""
    problems: list = []
    for name in ARTIFACT_METADATA_FIELDS:
        if name not in metadata or name not in artifact:
            continue
        header_value = metadata[name]
        object_value = artifact[name]
        if name in ("width", "height", "byteLength"):
            try:
                if int(header_value) != object_value:
                    problems.append(f"{name}-header-object-mismatch")
            except (TypeError, ValueError):
                problems.append(f"{name}-header-not-integer")
            continue
        if str(header_value) != str(object_value):
            problems.append(f"{name}-header-object-mismatch")
    return problems


def validate_artifact_object(artifact: object) -> tuple[dict, list]:
    """Validate the closed artifact metadata object of `GET /api/exports/{id}`."""
    problems: list = []
    if not isinstance(artifact, dict):
        return {}, ["artifact-not-object-or-null"]
    if set(artifact) != set(ARTIFACT_METADATA_FIELDS):
        problems.append("artifact-fields-not-exactly-closed-set")
    facts = dict(artifact)
    for name in ("width", "height", "byteLength"):
        if not isinstance(artifact.get(name), int) or isinstance(artifact.get(name), bool):
            problems.append(f"artifact-{name}-not-integer")
    if not _LOWER_HEX_64.match(str(artifact.get("sha256", ""))):
        problems.append("artifact-sha256-not-lowercase-hex-64")
    if parse_timestamp(artifact.get("expiresAt")) is None:
        problems.append("artifact-expiresAt-unparsable")
    if artifact.get("target") != DEVELOPMENT_TARGET:
        problems.append("artifact-target-not-development-tiff")
    if artifact.get("stage") != DEVELOP_STAGE:
        problems.append("artifact-stage-not-develop")
    return facts, problems


def validate_export_submission(payload: object) -> tuple[dict, list]:
    """Validate the 201/200 body of `POST /api/photos/{id}/exports`."""
    problems: list = []
    facts: dict = {}
    if not isinstance(payload, dict):
        return facts, ["payload-not-object"]
    if not isinstance(payload.get("exportId"), str) or not payload.get("exportId"):
        problems.append("exportId-missing")
    facts["exportId"] = payload.get("exportId")
    facts["state"] = payload.get("state")
    if payload.get("state") not in ("queued", "running"):
        problems.append("state-not-queued-or-running")
    if payload.get("target") != DEVELOPMENT_TARGET:
        problems.append("target-not-development-tiff")
    for key in ("recipeVersion", "sourceRevision"):
        if not isinstance(payload.get(key), str) or not payload.get(key):
            problems.append(f"{key}-missing")
    facts["receiptExpiresAt"] = payload.get("receiptExpiresAt")
    if payload.get("receiptExpiresAt") is not None:
        problems.append("receiptExpiresAt-not-null-while-active")
    facts["artifactExpiresAt"] = payload.get("artifactExpiresAt")
    if payload.get("artifactExpiresAt") is not None:
        problems.append("artifactExpiresAt-not-null-before-publication")
    return facts, problems


def validate_export_inspection(payload: object, export_id: str) -> tuple[dict, list]:
    """Validate `GET /api/exports/{id}` against the wire contract."""
    problems: list = []
    facts: dict = {}
    if not isinstance(payload, dict):
        return facts, ["payload-not-object"]
    if payload.get("exportId") != export_id:
        problems.append("exportId-mismatch")
    state = payload.get("state")
    facts["state"] = state
    if state not in _EXPORT_STATES:
        problems.append("state-outside-closed-set")
    if payload.get("target") != DEVELOPMENT_TARGET:
        problems.append("target-not-development-tiff")
    for key in ("recipeVersion", "sourceRevision", "bundleId"):
        if not isinstance(payload.get(key), str) or not payload.get(key):
            problems.append(f"{key}-missing")
    terminal = payload.get("terminalOutcome")
    facts["terminalOutcome"] = terminal
    expected_terminal = state if state in ("succeeded", "failed", "cancelled") else None
    if terminal != expected_terminal:
        problems.append("terminalOutcome-inconsistent-with-state")
    if state == "succeeded":
        if payload.get("receiptExpiresAt") is None:
            problems.append("receiptExpiresAt-null-after-settlement")
        artifact = payload.get("artifact")
        if artifact is None:
            problems.append("artifact-null-after-succeeded")
        else:
            artifact_facts, artifact_problems = validate_artifact_object(artifact)
            facts["artifact"] = artifact_facts
            problems.extend(artifact_problems)
    else:
        facts["artifact"] = payload.get("artifact")
        if payload.get("artifact") is not None and state in ("failed", "cancelled"):
            problems.append("artifact-present-without-success")
    facts["failureReason"] = payload.get("failureReason")
    if state == "failed" and not isinstance(payload.get("failureReason"), str):
        problems.append("failureReason-missing-after-failure")
    return facts, problems


def validate_preview_headers(
    metadata: dict,
    photo_id: str,
    expected_source_revision: str,
    expected_recipe_version: str,
) -> list:
    problems: list = []
    if metadata.get("photoId") != photo_id:
        problems.append("header-photoId-mismatch")
    if metadata.get("stage") != DEVELOP_STAGE:
        problems.append("header-stage-not-develop")
    if metadata.get("sourceRevision") != expected_source_revision:
        problems.append("header-sourceRevision-mismatch")
    if metadata.get("recipeVersion") != expected_recipe_version:
        problems.append("header-recipeVersion-mismatch")
    if metadata.get("displayTransform") != DISPLAY_TRANSFORM_IDENTITY:
        problems.append("header-displayTransform-unexpected")
    for name in ("width", "height", "byteLength"):
        try:
            int(metadata[name])
        except (KeyError, TypeError, ValueError):
            problems.append(f"header-{name}-not-integer")
    if not _LOWER_HEX_64.match(str(metadata.get("sha256", ""))):
        problems.append("header-sha256-not-lowercase-hex-64")
    if parse_timestamp(metadata.get("expiresAt")) is None:
        problems.append("header-expiresAt-unparsable")
    return problems


def validate_preview_body(metadata: dict, body: bytes) -> tuple[dict, list]:
    """Check the preview stream bytes against the declared typed metadata."""
    problems: list = []
    facts: dict = {"decode": None}
    if metadata.get("byteLength") is not None:
        try:
            if int(metadata["byteLength"]) != len(body):
                problems.append("byteLength-mismatch")
        except (TypeError, ValueError):
            problems.append("byteLength-not-integer")
    digest = sha256_hex(body)
    facts["sha256"] = digest
    if metadata.get("sha256") != digest:
        problems.append("sha256-mismatch")
    content_type = str(metadata.get("contentType", ""))
    facts["contentType"] = content_type
    width = _optional_int(metadata.get("width"))
    height = _optional_int(metadata.get("height"))
    if body[:2] == b"\xff\xd8":
        facts["decode"] = "jpeg-marker-walk"
        jpeg_facts, jpeg_problems = validate_jpeg(body)
        problems.extend(jpeg_problems)
        facts["width"] = jpeg_facts.get("width")
        facts["height"] = jpeg_facts.get("height")
        if jpeg_facts.get("width") != width or jpeg_facts.get("height") != height:
            problems.append("dimensions-mismatch")
    else:
        facts["decode"] = "unsupported-container"
        problems.append("preview-body-not-a-decodable-container")
    return facts, problems


def _optional_int(value: object) -> int | None:
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


_TIFF_TYPE_SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}


def validate_development_tiff(
    data: bytes,
    expected_width: int | None = None,
    expected_height: int | None = None,
    accepted_profile_digests: tuple[str, ...] = PINNED_SOURCE_PROFILE_DIGESTS,
) -> tuple[dict, list]:
    """Structurally validate a Development TIFF and its embedded profile."""
    problems: list = []
    facts: dict = {"decode": "ifd-structural"}
    if len(data) < 8:
        return facts, ["tiff-truncated-header"]
    byte_order = data[:2]
    if byte_order == b"II":
        endian = "<"
    elif byte_order == b"MM":
        endian = ">"
    else:
        return facts, ["tiff-byte-order-invalid"]
    magic = struct.unpack(endian + "H", data[2:4])[0]
    if magic != 42:
        problems.append("tiff-magic-invalid")
        return facts
    ifd_offset = struct.unpack(endian + "I", data[4:8])[0]
    if ifd_offset + 2 > len(data):
        return facts, ["tiff-ifd-out-of-range"]
    entry_count = struct.unpack(endian + "H", data[ifd_offset : ifd_offset + 2])[0]
    if ifd_offset + 2 + 12 * entry_count > len(data):
        return facts, ["tiff-ifd-out-of-range"]
    entries: dict[int, tuple[int, int, bytes]] = {}
    for index in range(entry_count):
        start = ifd_offset + 2 + 12 * index
        tag, kind, count = struct.unpack(endian + "HHI", data[start : start + 8])
        size = _TIFF_TYPE_SIZES.get(kind)
        if size is None:
            continue
        byte_count = size * count
        if byte_count <= 4:
            raw = data[start + 8 : start + 8 + byte_count]
        else:
            offset = struct.unpack(endian + "I", data[start + 8 : start + 12])[0]
            if offset + byte_count > len(data):
                problems.append(f"tiff-tag-{tag}-value-out-of-range")
                continue
            raw = data[offset : offset + byte_count]
        entries[tag] = (kind, count, raw)

    def unsigned(tag: int) -> int | None:
        entry = entries.get(tag)
        if entry is None:
            problems.append(f"tiff-tag-{tag}-missing")
            return None
        kind, count, raw = entry
        if kind == 3 and count == 1 and len(raw) >= 2:
            return struct.unpack(endian + "H", raw[:2])[0]
        if kind == 4 and count == 1 and len(raw) >= 4:
            return struct.unpack(endian + "I", raw[:4])[0]
        problems.append(f"tiff-tag-{tag}-unexpected-type")
        return None

    def short_list(tag: int, expected_count: int) -> list | None:
        entry = entries.get(tag)
        if entry is None:
            problems.append(f"tiff-tag-{tag}-missing")
            return None
        kind, count, raw = entry
        if kind != 3 or count != expected_count or len(raw) < 2 * expected_count:
            problems.append(f"tiff-tag-{tag}-unexpected-shape")
            return None
        return list(struct.unpack(endian + "H" * expected_count, raw[: 2 * expected_count]))

    width = unsigned(256)
    height = unsigned(257)
    bits = short_list(258, 3)
    compression = unsigned(259)
    photometric = unsigned(262)
    samples = unsigned(277)
    sample_format = short_list(339, 3)
    facts["width"] = width
    facts["height"] = height
    facts["bitsPerSample"] = bits
    facts["sampleFormat"] = sample_format
    facts["compression"] = compression
    facts["photometricInterpretation"] = photometric
    facts["samplesPerPixel"] = samples
    if samples != 3:
        problems.append("tiff-samples-per-pixel-not-3")
    if bits != [32, 32, 32]:
        problems.append("tiff-bits-per-sample-not-float32-rgb")
    if sample_format != [3, 3, 3]:
        problems.append("tiff-sample-format-not-ieee-float")
    if compression != 1:
        problems.append("tiff-compression-not-uncompressed")
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
    return facts, problems


def validate_jpeg(body: bytes) -> tuple[dict, list]:
    """Walk the JPEG marker structure far enough to trust container dimensions."""
    facts: dict = {"width": None, "height": None}
    problems: list = []
    if len(body) < 4 or body[:2] != b"\xff\xd8":
        return facts, ["jpeg-soi-missing"]
    position = 2
    saw_sof = False
    while position < len(body) - 1:
        if body[position] != 0xFF:
            problems.append("jpeg-marker-framing-broken")
            return facts
        while position < len(body) and body[position] == 0xFF:
            position += 1
        if position >= len(body):
            break
        marker = body[position]
        position += 1
        if marker in (0x01, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8):
            continue
        if marker == 0xD9:
            facts["sawEOI"] = True
            break
        if position + 2 > len(body):
            problems.append("jpeg-segment-truncated")
            return facts
        length = struct.unpack(">H", body[position : position + 2])[0]
        if length < 2 or position + length > len(body):
            problems.append("jpeg-segment-length-invalid")
            return facts
        segment = body[position + 2 : position + length]
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            if len(segment) >= 5:
                facts["height"] = struct.unpack(">H", segment[1:3])[0]
                facts["width"] = struct.unpack(">H", segment[3:5])[0]
            saw_sof = True
        if marker == 0xDA:
            facts["sawSOS"] = True
            break
        position += length
    if not saw_sof:
        problems.append("jpeg-sof-missing")
    if body[-2:] != b"\xff\xd9":
        problems.append("jpeg-eoi-missing")
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
    if parsed.scheme == "http" and parsed.hostname.lower() not in _LOOPBACK_HOSTS:
        raise InvocationRefused("base-url-http-requires-loopback-host")
    port = f":{parsed.port}" if parsed.port else ""
    return f"{parsed.scheme}://{parsed.hostname}{port}"


def read_token_file(path: Path) -> str:
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise InvocationRefused("token-file-not-regular")
        if metadata.st_mode & 0o022:
            raise InvocationRefused("token-file-writable-by-group-or-others")
        with path.open("r", encoding="utf-8") as stream:
            raw = stream.read(MAX_TOKEN_BYTES + 1)
    except InvocationRefused:
        raise
    except OSError as error:
        raise InvocationRefused("token-file-unreadable") from error
    if len(raw) > MAX_TOKEN_BYTES:
        raise InvocationRefused("token-file-too-large")
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
    if not output_dir.is_dir():
        raise InvocationRefused("output-dir-not-directory")
    resolved_output = output_dir.resolve()
    resolved_fixture = fixture.resolve()
    if resolved_fixture == resolved_output or resolved_output in resolved_fixture.parents:
        raise InvocationRefused("output-dir-must-not-contain-fixture")
    return output_dir


# ---------------------------------------------------------------------------
# HTTP client.
# ---------------------------------------------------------------------------


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
    """The authoritative error code of a contract refusal, if any."""
    if isinstance(payload, dict):
        code = payload.get("code")
        if isinstance(code, str) and code:
            return code
    return None


def require_success(response: Response, parsed: object, context: str, accepted=(200,)):
    """Turn a non-accepted response into a failure or a route-missing skip."""
    if response.status in accepted:
        if not isinstance(parsed, dict):
            raise AcceptanceFailure(f"{context}-payload-not-object", {})
        return
    code = structured_code(parsed)
    if response.status == 404 and code is None:
        raise RouteMissing(
            "route-not-deployed",
            {"path": context, "status": response.status},
        )
    raise AcceptanceFailure(
        f"{context}-refused",
        {"status": response.status, "code": code},
    )


# ---------------------------------------------------------------------------
# Runner.
# ---------------------------------------------------------------------------


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
    def __init__(
        self,
        *,
        base_url: str,
        token: str,
        fixture: Path,
        output_dir: Path,
        request_timeout: float = 30.0,
        settlement_timeout: float = 900.0,
        preview_timeout: float = 120.0,
        poll_interval: float = 2.0,
        accepted_profile_digests: tuple[str, ...] = PINNED_SOURCE_PROFILE_DIGESTS,
        expected_identities: dict | None = None,
        monotonic=time.monotonic,
    ):
        self.client = Client(base_url, token, timeout=request_timeout)
        self.fixture = fixture
        self.output_dir = output_dir
        self.settlement_timeout = settlement_timeout
        self.preview_timeout = preview_timeout
        self.poll_interval = poll_interval
        self.accepted_profile_digests = accepted_profile_digests
        self.expected_identities = expected_identities or {}
        self.monotonic = monotonic
        self.steps: list[StepRecord] = []
        self.identities: dict = {}
        self.capability: dict = {}
        self.photo_id: str | None = None
        self.source_revision: str | None = None
        self.observed_recipe: dict | None = None
        self.recipe_version: str | None = None
        self.controls: dict = {}
        self.export_id: str | None = None
        self.export_artifact: dict | None = None
        self.written_files: list[str] = []
        self.invariance_before: dict = {}

    # -- plumbing -----------------------------------------------------------

    def _step(self, name: str, function, gates: tuple = ()) -> StepRecord:
        record = StepRecord(name=name, startedAt=now_iso())
        mark = self.monotonic()
        log_mark = len(self.client.log)
        gate = self._gate_block(gates)
        if gate is not None:
            record.status = "skipped"
            record.reason = gate[0]
            record.detail = gate[1]
        else:
            try:
                record.detail = function() or {}
                record.status = "pass"
            except AcceptanceFailure as error:
                record.status = "fail"
                record.reason = error.reason
                record.detail = error.detail
            except RouteMissing as error:
                record.status = "skipped"
                record.reason = error.reason
                record.detail = error.detail
            except TransportFailure as error:
                record.status = "fail"
                record.reason = error.reason
                record.detail = error.detail
        record.durationSeconds = self.monotonic() - mark
        record.finishedAt = now_iso()
        record.requests = [
            {"method": entry.method, "path": entry.path, "status": entry.status}
            for entry in self.client.log[log_mark:]
        ]
        self.steps.append(record)
        return record

    def _gate_block(self, gates: tuple) -> tuple | None:
        for gate in gates:
            if isinstance(gate, tuple):
                name, condition, reason, detail = gate
                if not condition():
                    return (reason, detail)
                continue
            record = self._record(gate)
            if record is not None and record.status != "pass":
                return (
                    "prerequisite-not-passed",
                    {
                        "prerequisite": record.name,
                        "prerequisiteStatus": record.status,
                        "prerequisiteReason": record.reason,
                    },
                )
        return None

    def _record(self, name: str) -> StepRecord | None:
        for record in self.steps:
            if record.name == name:
                return record
        return None

    def _skip(self, reason: str, detail: dict | None = None):
        raise RouteMissing(reason, detail)

    # -- steps ---------------------------------------------------------------

    def _step_capability(self) -> dict:
        response, payload = self.client.request_json("GET", CAPABILITY_PATH)
        require_success(response, payload, "capability")
        facts, problems = validate_capability(payload)
        if problems:
            raise AcceptanceFailure("capability-invalid", {"problems": problems})
        self.capability = facts
        self.identities["capabilityState"] = facts["state"]
        self.identities["bundleId"] = facts["bundleId"]
        self.identities["incarnation"] = facts["incarnation"]
        expected_bundle = self.expected_identities.get("bundleSha256")
        if expected_bundle and expected_bundle != facts["bundleId"]:
            raise AcceptanceFailure(
                "capability-bundle-mismatch",
                {"expected": expected_bundle, "observed": facts["bundleId"]},
            )
        detail = {
            "state": facts["state"],
            "bundleId": facts["bundleId"],
            "stages": facts["stages"],
        }
        return detail

    def _processing_ready(self) -> bool:
        return self.capability.get("state") == "ready"

    def _develop_ready(self) -> bool:
        return self.capability.get("stages", {}).get(DEVELOP_STAGE) == "ready"

    def _require_processing_ready(self):
        if not self._processing_ready() or not self._develop_ready():
            self._skip(
                "processing-not-ready",
                {
                    "capabilityState": self.capability.get("state"),
                    "developStage": self.capability.get("stages", {}).get(DEVELOP_STAGE),
                },
            )

    def _step_resolve_photo(self) -> dict:
        matches = []
        seen = 0
        path = PHOTO_QUERIES_PATH
        cursor = None
        for _page in range(MAX_QUERY_PAGES):
            if cursor is None:
                response, payload = self.client.request_json(
                    "POST",
                    path,
                    payload={"source": "all", "kind": "raw", "available": True, "limit": 200},
                )
                require_success(response, payload, "photo-query")
            else:
                response, payload = self.client.request_json("GET", f"{path}/{cursor}")
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
        return {"photoId": self.photo_id, "photosSeen": seen}

    def _step_recipe_read(self) -> dict:
        response, payload = self.client.request_json(
            "GET", EDIT_RECIPE_PATH.format(id=self.photo_id)
        )
        require_success(response, payload, "recipe-read")
        facts, problems = validate_recipe_read(payload, self.photo_id)
        if problems:
            raise AcceptanceFailure("recipe-read-invalid", {"problems": problems})
        if facts["sourceSupport"] == "unavailable":
            raise AcceptanceFailure(
                "source-unavailable",
                {"supportReason": payload.get("supportReason")},
            )
        if facts["sourceSupport"] == "unsupported":
            raise AcceptanceFailure(
                "source-unsupported",
                {"profiles": self.capability.get("profiles")},
            )
        self.source_revision = facts["sourceRevision"]
        self.observed_recipe = facts.get("recipe")
        self.controls = facts.get("controls") or {}
        recipe = facts.get("recipe") or {}
        self.recipe_version = recipe.get("recipeVersion")
        self.identities["sourceRevision"] = self.source_revision
        self.identities["recipeVersionObserved"] = self.recipe_version
        return {
            "sourceRevision": self.source_revision,
            "hadSavedRecipe": facts.get("recipe") is not None,
            "processingAvailable": payload.get("processingAvailable"),
        }

    def _baseline_exposure(self) -> float | None:
        if self.observed_recipe and isinstance(
            self.observed_recipe.get("exposureEv"), (int, float)
        ):
            return float(self.observed_recipe["exposureEv"])
        return None

    def _guarded_save(self, purpose: str, exposure: float, expected_version: str | None) -> dict:
        body = build_save_body(
            new_request_identity(purpose), expected_version, self.source_revision, exposure
        )
        modes = self.controls.get("whiteBalanceModes") or []
        if "as-shot" not in modes:
            raise AcceptanceFailure(
                "as-shot-mode-not-admitted",
                {"whiteBalanceModes": modes},
            )
        response, payload = self.client.request_json(
            "POST", EDIT_RECIPE_PATH.format(id=self.photo_id), payload=body
        )
        require_success(response, payload, "recipe-save")
        facts, problems = validate_save_response(payload)
        if problems:
            raise AcceptanceFailure("recipe-save-invalid", {"problems": problems})
        if facts["outcome"] != "saved":
            raise AcceptanceFailure(
                "recipe-save-outcome-unexpected",
                {"expected": "saved", "observed": facts["outcome"]},
            )
        if facts.get("sourceRevision") != self.source_revision:
            raise AcceptanceFailure(
                "source-revision-changed",
                {
                    "expected": self.source_revision,
                    "observed": facts.get("sourceRevision"),
                },
            )
        self.recipe_version = facts["recipeVersion"]
        return facts

    def _step_save_exposure(self) -> dict:
        target = choose_exposure(self.controls, self._baseline_exposure())
        facts = self._guarded_save("save", target, self.recipe_version)
        self.identities["recipeVersionAfterSave"] = self.recipe_version
        return {"exposureEv": target, "recipeVersion": facts["recipeVersion"]}

    def _step_save_undo(self) -> dict:
        baseline = self._baseline_exposure()
        target = exposure_on_grid(self.controls, 0.0 if baseline is None else baseline)
        facts = self._guarded_save("undo", target, self.recipe_version)
        self.identities["recipeVersionAfterUndo"] = self.recipe_version
        note = (
            None if baseline is not None
            else "no recipe existed before the run; the reversal saved the baseline exposure as new editing intent"
        )
        detail = {"exposureEv": target, "recipeVersion": facts["recipeVersion"]}
        if note:
            detail["note"] = note
        return detail

    def _step_edit_preview(self) -> dict:
        self._require_processing_ready()
        path = EDIT_PREVIEW_PATH.format(id=self.photo_id, stage=DEVELOP_STAGE)
        deadline = self.monotonic() + self.preview_timeout
        polls = 0
        while True:
            response = self.client.request("GET", path, max_bytes=self.client.max_download_bytes)
            if response.status == 202:
                polls += 1
                try:
                    payload = json.loads(response.data)
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    raise AcceptanceFailure(
                        "preview-admission-not-json", {"error": type(error).__name__}
                    ) from error
                state = payload.get("state") if isinstance(payload, dict) else None
                if state not in ("queued", "running"):
                    raise AcceptanceFailure(
                        "preview-admission-invalid",
                        {"state": state},
                    )
                if self.monotonic() >= deadline:
                    raise AcceptanceFailure("preview-render-timeout", {"polls": polls})
                time.sleep(self.poll_interval)
                continue
            if response.status == 404:
                code = structured_code(self._safe_json(response))
                if code is None:
                    self._skip("route-not-deployed", {"path": "edit-preview", "status": 404})
                raise AcceptanceFailure("preview-refused", {"status": 404, "code": code})
            if response.status != 200:
                raise AcceptanceFailure(
                    "preview-refused",
                    {"status": response.status, "code": structured_code(self._safe_json(response))},
                )
            break
        metadata, problems = collect_metadata_headers(response.headers, PREVIEW_METADATA_FIELDS)
        problems.extend(
            validate_preview_headers(
                metadata, self.photo_id, self.source_revision, self.recipe_version
            )
        )
        if problems:
            raise AcceptanceFailure("preview-metadata-invalid", {"problems": problems})
        body_facts, body_problems = validate_preview_body(metadata, response.data)
        if body_problems:
            raise AcceptanceFailure(
                "preview-body-invalid",
                {"problems": body_problems, "facts": body_facts},
            )
        expiry = parse_timestamp(metadata.get("expiresAt"))
        if expiry is not None and expiry <= datetime.now(timezone.utc):
            problems.append("preview-already-expired")
        if problems:
            raise AcceptanceFailure("preview-metadata-invalid", {"problems": problems})
        return {
            "contentType": body_facts.get("contentType"),
            "width": body_facts.get("width"),
            "height": body_facts.get("height"),
            "sha256": body_facts.get("sha256"),
            "displayTransform": metadata.get("displayTransform"),
            "decode": body_facts.get("decode"),
        }

    @staticmethod
    def _safe_json(response: Response) -> object:
        try:
            return json.loads(response.data)
        except (UnicodeDecodeError, json.JSONDecodeError):
            return None

    def _step_submit_export(self) -> dict:
        self._require_processing_ready()
        body = {
            "requestId": new_request_identity("export"),
            "expectedRecipeVersion": self.recipe_version,
            "expectedSourceRevision": self.source_revision,
            "target": DEVELOPMENT_TARGET,
        }
        response, payload = self.client.request_json(
            "POST", PHOTO_EXPORTS_PATH.format(id=self.photo_id), payload=body
        )
        require_success(response, payload, "export-submit", accepted=(201,))
        facts, problems = validate_export_submission(payload)
        if problems:
            raise AcceptanceFailure(
                "export-submit-invalid", {"problems": problems, "facts": facts}
            )
        self.export_id = facts["exportId"]
        self.identities["exportId"] = self.export_id
        return {"exportId": self.export_id, "state": facts["state"]}

    def _step_export_settlement(self) -> dict:
        self._require_processing_ready()
        deadline = self.monotonic() + self.settlement_timeout
        path = EXPORT_PATH.format(id=self.export_id)
        while True:
            response, payload = self.client.request_json("GET", path)
            require_success(response, payload, "export-inspect")
            facts, problems = validate_export_inspection(payload, self.export_id)
            state = facts.get("state")
            if state in ("succeeded", "failed", "cancelled"):
                break
            if self.monotonic() >= deadline:
                raise AcceptanceFailure(
                    "export-settlement-timeout",
                    {"state": state, "problems": problems},
                )
            time.sleep(self.poll_interval)
        expected_bundle = self.capability.get("bundleId")
        if expected_bundle and payload.get("bundleId") != expected_bundle:
            problems.append("export-bundle-mismatch")
        if facts["state"] != "succeeded":
            raise AcceptanceFailure(
                "export-not-succeeded",
                {
                    "state": facts["state"],
                    "terminalOutcome": facts.get("terminalOutcome"),
                    "failureReason": facts.get("failureReason"),
                },
            )
        if problems:
            raise AcceptanceFailure("export-inspect-invalid", {"problems": problems})
        self.export_artifact = facts["artifact"]
        self.identities["exportState"] = facts["state"]
        self.identities["exportBundleId"] = payload.get("bundleId")
        self.identities["receiptExpiresAt"] = payload.get("receiptExpiresAt")
        return {
            "state": facts["state"],
            "artifact": {
                key: self.export_artifact.get(key)
                for key in ("exportId", "sha256", "byteLength", "width", "height", "expiresAt")
            },
        }

    def _step_download_artifact(self) -> dict:
        self._require_processing_ready()
        path = EXPORT_ARTIFACT_PATH.format(id=self.export_id)
        declared = None
        if self.export_artifact:
            declared = self.export_artifact.get("byteLength")
        limit = (
            int(declared) + DOWNLOAD_SLACK_BYTES
            if isinstance(declared, int)
            else self.client.max_download_bytes
        )
        response = self.client.request("GET", path, max_bytes=limit)
        if response.status == 404:
            code = structured_code(self._safe_json(response))
            if code is None:
                self._skip("route-not-deployed", {"path": "export-artifact", "status": 404})
        if response.status != 200:
            raise AcceptanceFailure(
                "artifact-download-refused",
                {"status": response.status, "code": structured_code(self._safe_json(response))},
            )
        metadata, problems = collect_metadata_headers(response.headers, ARTIFACT_METADATA_FIELDS)
        if metadata.get("target") != DEVELOPMENT_TARGET:
            problems.append("header-target-not-development-tiff")
        if metadata.get("stage") != DEVELOP_STAGE:
            problems.append("header-stage-not-develop")
        if metadata.get("contentType") != DEVELOPMENT_CONTENT_TYPE:
            problems.append("header-contentType-not-development-tiff")
        digest = sha256_hex(response.data)
        if metadata.get("sha256") != digest:
            problems.append("header-sha256-mismatch")
        byte_length = _optional_int(metadata.get("byteLength"))
        if byte_length != len(response.data):
            problems.append("header-byteLength-mismatch")
        expiry = parse_timestamp(metadata.get("expiresAt"))
        if expiry is None:
            problems.append("header-expiresAt-unparsable")
        elif expiry <= datetime.now(timezone.utc):
            problems.append("artifact-already-expired")
        if self.export_artifact is not None:
            problems.extend(header_object_mismatches(metadata, self.export_artifact))
        tiff_facts, tiff_problems = validate_development_tiff(
            response.data,
            expected_width=_optional_int(metadata.get("width")),
            expected_height=_optional_int(metadata.get("height")),
            accepted_profile_digests=self.accepted_profile_digests,
        )
        problems.extend(tiff_problems)
        profile_identity = metadata.get("profileIdentity")
        facts = {
            "sha256": digest,
            "byteLength": len(response.data),
            "contentType": metadata.get("contentType"),
            "expiresAt": metadata.get("expiresAt"),
            "profileIdentity": profile_identity,
            "tiff": tiff_facts,
        }
        if profile_identity and tiff_facts.get("profileSha256"):
            if profile_identity != tiff_facts["profileSha256"]:
                both_pinned = (
                    profile_identity in self.accepted_profile_digests
                    and tiff_facts.get("profileAccepted") is True
                )
                if not both_pinned:
                    problems.append("profileIdentity-does-not-match-embedded-profile")
                else:
                    facts["profileIdentityRule"] = "pinned-pair"
            else:
                facts["profileIdentityRule"] = "embedded-digest"
        if problems:
            raise AcceptanceFailure(
                "artifact-invalid", {"problems": problems, "facts": facts}
            )
        destination = self.output_dir / f"{self.export_id}.tiff"
        if destination.exists():
            raise AcceptanceFailure(
                "artifact-path-occupied", {"path": str(destination)}
            )
        destination.write_bytes(response.data)
        self.written_files.append(str(destination))
        self.identities["artifact"] = {
            "exportId": self.export_id,
            "sha256": digest,
            "byteLength": len(response.data),
            "width": tiff_facts.get("width"),
            "height": tiff_facts.get("height"),
            "profileIdentity": profile_identity,
            "expiresAt": metadata.get("expiresAt"),
        }
        return facts

    def _step_film_stage(self) -> dict:
        self._skip(
            "film-stage-not-implemented",
            {
                "owner": "issue-332",
                "observedCapabilityStage": self.capability.get("stages", {}).get("film"),
                "note": (
                    "The finished-jpeg Film stage is not implemented yet; the runner "
                    "will cover it once Issue #332 lands."
                ),
            },
        )

    def _step_invariance_after(self) -> dict:
        snapshots = self._current_snapshots()
        changes = invariance_changes(self.invariance_before, snapshots)
        if changes:
            raise AcceptanceFailure("original-mutated", {"changes": changes})
        return {"checked": sorted(snapshots), "unchanged": True}

    def _current_snapshots(self) -> dict:
        snapshots = {}
        candidates = [self.fixture] + [
            path for path in external_xmp_sidecars(self.fixture) if path.exists()
        ]
        for path in candidates:
            snapshots[str(path)] = snapshot_original(path)
        return snapshots

    def _invariance_before(self) -> None:
        if not self.fixture.is_file():
            raise InvocationRefused("fixture-not-regular-file")
        try:
            self.invariance_before = self._current_snapshots()
        except OSError as error:
            raise InvocationRefused("fixture-unreadable") from error

    # -- orchestration -------------------------------------------------------

    def run(self) -> dict:
        started = self.monotonic()
        self._invariance_before()
        self._step("capability", self._step_capability)
        self._step("resolve-photo", self._step_resolve_photo, gates=("capability",))
        self._step("read-recipe", self._step_recipe_read, gates=("resolve-photo",))
        self._step("save-exposure", self._step_save_exposure, gates=("read-recipe",))
        self._step("save-undo", self._step_save_undo, gates=("save-exposure",))
        self._step(
            "edit-preview",
            self._step_edit_preview,
            gates=(
                "save-undo",
                (
                    "develop-ready",
                    self._develop_ready,
                    "develop-stage-not-ready",
                    {
                        "capabilityState": self.capability.get("state"),
                        "developStage": self.capability.get("stages", {}).get(DEVELOP_STAGE),
                    },
                ),
            ),
        )
        self._step(
            "submit-export",
            self._step_submit_export,
            gates=(
                "save-undo",
                (
                    "develop-ready",
                    self._develop_ready,
                    "develop-stage-not-ready",
                    {
                        "capabilityState": self.capability.get("state"),
                        "developStage": self.capability.get("stages", {}).get(DEVELOP_STAGE),
                    },
                ),
            ),
        )
        self._step("export-settlement", self._step_export_settlement, gates=("submit-export",))
        self._step("download-artifact", self._step_download_artifact, gates=("export-settlement",))
        self._step("film-stage", self._step_film_stage)
        self._step("original-invariance", self._step_invariance_after)
        return self.report(started)

    def report(self, started: float) -> dict:
        failed = [record.name for record in self.steps if record.status == "fail"]
        skipped = [record for record in self.steps if record.status == "skipped"]
        not_run = [
            {"step": record.name, "reason": record.reason, "detail": record.detail}
            for record in skipped
        ]
        film_observed = self.capability.get("stages", {}).get("film")
        if failed:
            status = "failed"
        elif any(record.name != "film-stage" for record in skipped):
            status = "blocked"
        else:
            status = "passed"
        return {
            "scope": "photo-development-acceptance-workflow",
            "issue": 334,
            "status": status,
            "startedAt": self.steps[0].startedAt if self.steps else now_iso(),
            "finishedAt": now_iso(),
            "durationSeconds": round(self.monotonic() - started, 3),
            "acknowledgement": {
                "acceptanceInstance": True,
                "flag": "--i-acknowledge-this-is-an-acceptance-instance",
            },
            "target": {"baseUrl": self.client.base_url},
            "operatorSuppliedIdentities": self.expected_identities,
            "identities": self.identities,
            "fixture": self.invariance_before.get(str(self.fixture), {}),
            "sidecars": [
                snapshot
                for path, snapshot in self.invariance_before.items()
                if path != str(self.fixture)
            ],
            "steps": [record.as_dict() for record in self.steps],
            "notRun": not_run,
            "counters": {
                "requests": len(self.client.log),
                "stepsPassed": sum(1 for record in self.steps if record.status == "pass"),
                "stepsFailed": len(failed),
                "stepsSkipped": len(skipped),
                "writtenFiles": len(self.written_files),
            },
            "writtenFiles": list(self.written_files),
            "filmStage": {
                "covered": False,
                "ownerIssue": 332,
                "observedCapabilityStage": film_observed,
            },
        }


def render_summary(report: dict) -> str:
    lines = []
    for record in report["steps"]:
        marker = {"pass": "PASS", "fail": "FAIL", "skipped": "SKIP"}[record["status"]]
        suffix = f" ({record['reason']})" if record.get("reason") else ""
        lines.append(f"[{marker}] {record['name']}{suffix}")
    lines.append(f"Status: {report['status']}")
    if report["filmStage"]["covered"] is False:
        lines.append(
            "Film (finished-jpeg) stage not covered: owned by Issue #332, not implemented yet."
        )
    for entry in report["notRun"]:
        if entry["step"] != "film-stage":
            lines.append(f"Could not run {entry['step']}: {entry['reason']}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Command line.
# ---------------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base-url", required=True, help="Deployment base URL (https, or http on loopback).")
    parser.add_argument("--token-file", required=True, type=Path, help="File holding the bearer token.")
    parser.add_argument("--fixture", required=True, type=Path, help="Approved-profile RAW fixture path.")
    parser.add_argument("--output-dir", required=True, help="Private directory for downloaded artifacts.")
    parser.add_argument("--i-acknowledge-this-is-an-acceptance-instance", action="store_true")
    parser.add_argument("--expected-instance")
    parser.add_argument("--expected-policy")
    parser.add_argument("--expected-bundle-sha256")
    parser.add_argument("--request-timeout", type=float, default=30.0)
    parser.add_argument("--settlement-timeout", type=float, default=900.0)
    parser.add_argument("--preview-timeout", type=float, default=120.0)
    parser.add_argument("--poll-interval", type=float, default=2.0)
    return parser.parse_args(argv)


def refused_report(reason: str) -> dict:
    return {
        "scope": "photo-development-acceptance-workflow",
        "issue": 334,
        "status": "refused",
        "reason": reason,
    }


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(argv)
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
            "instance": arguments.expected_instance,
            "policy": arguments.expected_policy,
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
