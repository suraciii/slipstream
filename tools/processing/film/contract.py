"""Closed metadata boundary for the shared measurement/qualified Film adapter."""

from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import sys

# This target contains only separately locked executor dependencies. It must not
# replace any distribution or top-level module in the numerical runtime.
VALIDATOR_DEPS = Path("/opt/film-validator-deps")
if VALIDATOR_DEPS.is_dir():
    sys.path.insert(0, str(VALIDATOR_DEPS))

from jsonschema import Draft202012Validator
from referencing import Registry, Resource


MAX_BYTES = 16 * 1024
U64_MAX = (1 << 64) - 1
NUMERICAL_BUNDLE = "0bf4af15d4f5323d060d4e6543e0e97d1a2fe6014d27c1cd81db440e4f46a152"
RECIPE = "a2dabe2581df7ed97b2af2ffe9bd4a80e579e0f49fd9627af49d86267a33ea7d"
INPUT_ICC = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
OUTPUT_ICC = "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21"
PROCEDURE = "film-once-empty-cache-v1"
GRANT = Path("/input/grant.json")


class ContractError(Exception):
    def __init__(self, detail="plan-rejected"):
        super().__init__(detail)
        self.detail = detail


def _reject_number(_):
    raise ContractError()


def _integer(token):
    if not token.isascii() or not token.isdecimal():
        raise ContractError()
    value = int(token)
    if value > U64_MAX:
        raise ContractError()
    return value


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ContractError()
        result[key] = value
    return result


def _check_values(value, depth=0):
    if depth > 64:
        raise ContractError()
    if value is None or type(value) is bool:
        return
    if type(value) is int:
        if not 0 <= value <= U64_MAX:
            raise ContractError()
    elif type(value) is str:
        if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
            raise ContractError()
    elif type(value) is list:
        for item in value:
            _check_values(item, depth + 1)
    elif type(value) is dict:
        for key, item in value.items():
            if type(key) is not str:
                raise ContractError()
            _check_values(key, depth + 1)
            _check_values(item, depth + 1)
    else:
        raise ContractError()


def parse_json(data):
    if not 0 < len(data) <= MAX_BYTES:
        raise ContractError()
    try:
        value = json.loads(
            data.decode("utf-8"), object_pairs_hook=_object,
            parse_int=_integer, parse_float=_reject_number,
            parse_constant=_reject_number,
        )
        _check_values(value)
        return value
    except (UnicodeError, ValueError, RecursionError) as error:
        raise ContractError() from error


def canonical_bytes(value):
    _check_values(value)
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False,
        sort_keys=True, separators=(",", ":"),
    ).encode("utf-8")


def digest(value):
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def schema_path(version):
    names = {
        2: ("schema.json", "processing-film-measurement.schema.json"),
        3: ("envelope-schema.json", "processing-film-envelope.schema.json"),
    }
    if type(version) is not int or version not in names:
        raise ContractError()
    packaged, source = names[version]
    installed = Path("/opt/film-measurement") / packaged
    if not installed.is_file():
        installed = Path(__file__).resolve().parents[3] / "design/schemas" / source
    return installed


@lru_cache(maxsize=2)
def schema(version=2):
    return json.loads(schema_path(version).read_text())


@lru_cache(maxsize=1)
def schema_registry():
    # Registry's default retrieval rejects unknown resources. Never fetch a URI
    # from the network or turn a schema reference into an arbitrary file read.
    measurement = Resource.from_contents(schema(2))
    envelope_path = schema_path(3)
    return Registry().with_resources([
        (schema_path(2).as_uri(), measurement),
        (envelope_path.as_uri(), Resource.from_contents(schema(3))),
        (envelope_path.with_name("processing-film-measurement.schema.json").as_uri(), measurement),
    ])


@lru_cache(maxsize=16)
def validator(name, version=2):
    return Draft202012Validator(
        {"$ref": schema_path(version).as_uri() + "#/$defs/" + name},
        registry=schema_registry(),
    )


def validate(name, value, *, version=2):
    _check_values(value)
    if not validator(name, version).is_valid(value):
        raise ContractError()


def read_grant():
    expected = os.environ.get("SLIPSTREAM_FILM_GRANT_SHA256", "")
    if re.fullmatch("[0-9a-f]{64}", expected) is None:
        raise ContractError()
    descriptor = os.open(GRANT, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != 0
                or stat.S_IMODE(metadata.st_mode) != 0o444
                or metadata.st_nlink != 1 or not 0 < metadata.st_size <= MAX_BYTES):
            raise ContractError()
        data = stream.read(MAX_BYTES + 1)
    if len(data) != metadata.st_size or hashlib.sha256(data).hexdigest() != expected:
        raise ContractError()
    grant = parse_json(data)
    if type(grant) is not dict:
        raise ContractError()
    version = grant.get("version")
    expected_kind = {2: "film-measurement-grant", 3: "film-qualified-grant"}
    if (type(version) is not int or version not in expected_kind
            or grant.get("kind") != expected_kind[version]):
        raise ContractError()
    validate("EngineGrant", grant, version=version)
    if canonical_bytes(grant) != data:
        raise ContractError()
    if (grant["numerical_bundle"] != NUMERICAL_BUNDLE or grant["recipe"] != RECIPE
            or grant["input_icc_sha256"] != INPUT_ICC
            or grant["output_icc_sha256"] != OUTPUT_ICC
            or grant["procedure"] != PROCEDURE):
        raise ContractError()
    return grant


def write_producer(value, descriptor=3):
    validate("ProducerResult", value)
    body = canonical_bytes(value)
    if len(body) > MAX_BYTES - 4:
        raise ContractError()
    frame = memoryview(struct.pack(">I", len(body)) + body)
    try:
        while frame:
            written = os.write(descriptor, frame)
            if written == 0:
                raise BrokenPipeError()
            frame = frame[written:]
    finally:
        os.close(descriptor)
