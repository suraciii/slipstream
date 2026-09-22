import copy
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import contract


def grant():
    plans = {}
    for name, model, fixed, per_pixel, destination, allowance in (
        ("gamut", "cam16ucs-srgb-f64-v1", 67108864, 2048, 7752, 603979776),
        ("cctf", "srgb-cctf-f64-v1", 4194304, 256, 7752, 603979776),
        ("jpeg", "jpeg-uint8-rows-v1", 1048576, 64, 0, 17825792),
    ):
        plans[name] = {"model": model, "allowance_bytes": allowance,
                       "scratch_bytes": fixed + per_pixel * 323,
                       "batch_pixels": 323, "destination_bytes": destination}
    return {
        "version": 2, "kind": "film-measurement-grant", "launch_id": "1" * 32,
        "manifest": "2" * 64, "bundle": "3" * 64,
        "numerical_bundle": contract.NUMERICAL_BUNDLE, "recipe": contract.RECIPE,
        "procedure": contract.PROCEDURE, "input_icc_sha256": contract.INPUT_ICC,
        "output_icc_sha256": contract.OUTPUT_ICC,
        "fixture": {"id": "4" * 32, "width": 19, "height": 17,
                    "source": {"kind": "synthetic-rgb", "generator": "linear-rgb-f32-v1",
                               "pattern": "gradient", "seed": 0},
                    "reference": {"input_pixels_sha256": "5" * 64,
                                  "film_pixels_sha256": "6" * 64, "jpeg_sha256": "7" * 64,
                                  "jpeg_bytes": 100, "evidence_sha256": "8" * 64}},
        "plan": {"formula": "film-live-storage-v1", "width": 19, "height": 17,
                 "source_cache_bytes": 0, "storage_reserve_bytes": 4294967296,
                 "prediction": {"status": "unqualified", "known_required_bytes": 4294967296,
                                "known_terms_exceed_limit": False,
                                "missing": [{"stage": "scan", "term": "native"}]}, **plans},
    }


class ContractTests(unittest.TestCase):
    def test_canonical_cross_language_vectors(self):
        installed = Path("/opt/film-checks/canonical-vectors.json")
        if not installed.exists():
            installed = Path(__file__).resolve().parents[3] / "design/schemas/processing-film-measurement-canonical-vectors.json"
        vectors = json.loads(installed.read_text())
        for case in vectors["cases"]:
            with self.subTest(case=case["name"]):
                if "error" in case:
                    with self.assertRaises(contract.ContractError):
                        contract.parse_json(case["input_json"].encode())
                else:
                    value = contract.parse_json(case["input_json"].encode())
                    self.assertEqual(contract.canonical_bytes(value).hex(), case["canonical_utf8_hex"])
                    self.assertEqual(contract.digest(value), case["sha256"])

    def test_parser_rejects_ambiguous_or_unbounded_values(self):
        for data in [b'{"a":1,"a":2}', b'-0', b'-1', b'1.0', b'1e0', b'NaN',
                     b'18446744073709551616', b'"\xff"', b'{}{}', b' ',
                     b'"' + b'x' * 16384 + b'"', b'[' * 66 + b'0' + b']' * 66]:
            with self.subTest(data=data[:40]):
                with self.assertRaises(contract.ContractError):
                    contract.parse_json(data)

    def test_schema_rejects_unknown_authority_and_newline_identities(self):
        value = grant()
        contract.validate("EngineGrant", value)
        for key in ("launch_id", "manifest", "bundle"):
            for suffix in ("\n", "\r\n"):
                invalid = copy.deepcopy(value)
                invalid[key] += suffix
                with self.assertRaises(contract.ContractError):
                    contract.validate("EngineGrant", invalid)
        value["command"] = "/bin/sh"
        with self.assertRaises(contract.ContractError):
            contract.validate("EngineGrant", value)

    def test_producer_pipe_is_one_exact_frame_and_eof(self):
        value = {"version": 2, "kind": "film-producer-result", "outcome": "engine-failed",
                 "detail": "plan-rejected", "launch_id": "1" * 32,
                 "manifest": "2" * 64, "plan_sha256": "3" * 64}
        read_fd, write_fd = os.pipe()
        try:
            contract.write_producer(value, write_fd)
            with os.fdopen(read_fd, "rb") as stream:
                data = stream.read()
            count = struct.unpack(">I", data[:4])[0]
            self.assertEqual(count, len(data) - 4)
            self.assertEqual(contract.parse_json(data[4:]), value)
        finally:
            for descriptor in (read_fd, write_fd):
                try:
                    os.close(descriptor)
                except OSError:
                    pass
        value["jpeg_sha256"] = "0" * 64
        with self.assertRaises(contract.ContractError):
            contract.validate("ProducerResult", value)

    def test_grant_requires_sealed_regular_file_and_exact_canonical_digest(self):
        data = contract.canonical_bytes(grant())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "grant.json"
            path.write_bytes(data)
            path.chmod(0o444)
            expected = hashlib.sha256(data).hexdigest()
            with patch.object(contract, "GRANT", path), patch.dict(os.environ, SLIPSTREAM_FILM_GRANT_SHA256=expected):
                if os.getuid() == 0:
                    self.assertEqual(contract.read_grant(), grant())
                else:
                    with self.assertRaises(contract.ContractError):
                        contract.read_grant()
                path.chmod(0o644)
                with self.assertRaises(contract.ContractError):
                    contract.read_grant()
                path.unlink()
                path.symlink_to(Path(directory) / "absent")
                with self.assertRaises(OSError):
                    contract.read_grant()


if __name__ == "__main__":
    unittest.main()
