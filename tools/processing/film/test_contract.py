import copy
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
from referencing.exceptions import NoSuchResource

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


def qualified_grant():
    """Synthetic syntax fixture only; no value asserts approved qualification."""
    value = grant()
    value.update(version=3, kind="film-qualified-grant")
    plan = value["plan"]
    del plan["prediction"]
    plan.update(
        formula="film-total-envelope-v1", inventory="film-known-storage-local-v1",
        envelope_sha256="a" * 64, evidence_sha256="b" * 64, environment_sha256="c" * 64,
        known_required_bytes=4294967296 + max(65536, *(plan[name]["scratch_bytes"]
                                                       for name in ("gamut", "cctf", "jpeg"))),
        empirical_ceiling_bytes=1073741824, safety_reserve_bytes=67108864,
        required_bytes=5435817984, attempt_limit_bytes=8589934592,
        missing=[{"stage": stage, "term": term}
                 for stage in contract.schema()["$defs"]["Stage"]["enum"]
                 for term in contract.schema()["$defs"]["Term"]["enum"]
                 if not (stage in ("staging", "validation") and term == "owned-arrays")],
    )
    return value


class ContractTests(unittest.TestCase):
    def test_qualified_canonical_vectors_use_only_packaged_schema_references(self):
        installed = Path("/opt/film-checks/envelope-vectors.json")
        if not installed.exists():
            installed = Path(__file__).resolve().parents[3] / "design/schemas/processing-film-envelope-vectors.json"
        vectors = json.loads(installed.read_text())
        for case in vectors["cases"]:
            with self.subTest(case=case["name"]):
                value = contract.parse_json(case["input_json"].encode())
                contract.validate(case["definition"], value, version=3)
                self.assertEqual(contract.canonical_bytes(value).hex(), case["canonical_utf8_hex"])
                self.assertEqual(contract.digest(value), case["sha256"])
        for uri in ("https://untrusted.invalid/schema.json", "file:///etc/passwd"):
            with self.assertRaises(NoSuchResource):
                contract.schema_registry().get_or_retrieve(uri)

    def test_grant_versions_and_plan_kinds_cannot_be_mixed(self):
        for value in (grant(), qualified_grant()):
            contract.validate("EngineGrant", value, version=value["version"])
            for field, replacement in (("version", 5 - value["version"]),
                                       ("kind", "film-measurement-grant" if value["version"] == 3
                                        else "film-qualified-grant"),
                                       ("plan", grant()["plan"] if value["version"] == 3
                                        else qualified_grant()["plan"])):
                invalid = copy.deepcopy(value)
                invalid[field] = replacement
                with self.assertRaises(contract.ContractError):
                    contract.validate("EngineGrant", invalid, version=value["version"])
        for field in ("envelope_sha256", "evidence_sha256", "environment_sha256"):
            invalid = qualified_grant()
            invalid["plan"][field] += "\n"
            with self.assertRaises(contract.ContractError):
                contract.validate("EngineGrant", invalid, version=3)

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

    def test_terminal_receipt_io_syntax_is_shared_by_both_profiles(self):
        for version, fixture in ((2, grant()), (3, qualified_grant())):
            snapshot = dict(
                cgroup_path="/sys/fs/cgroup/example.slice", cgroup_inode=1,
                unit_invocation="1" * 32, launch_id=fixture["launch_id"],
                container_id="2" * 64, attempt_unit="example.slice",
                incarnation="3" * 32, sequence=1,
                memory_peak_raw="100\n", memory_max_raw="8589934592\n",
                memory_swap_current_raw="0\n", memory_swap_max_raw="0\n",
                memory_events_raw="oom 0\noom_kill 0\noom_group_kill 0\n",
                memory_events_local_raw="oom 0\noom_kill 0\noom_group_kill 0\n")
            receipt = dict(
                incarnation=snapshot["incarnation"], sequence=1,
                workload=dict(kind="film-fixture", fixture_id=fixture["fixture"]["id"]),
                policy="4" * 64, bundle=fixture["bundle"], catalogue="5" * 64,
                manifest=fixture["manifest"], state="settled", phase="execution-finished",
                cancellation_requested=False, accepted_at_unix_ms=1, deadline_unix_ms=2,
                outcome="interrupted", detail=None, runtime=None,
                limits=dict(memory_bytes=8589934592, swap_bytes=0, cpu_quota_us=400000,
                            cpu_period_us=100000, tasks=256, storage_bytes=4294967296,
                            storage_inodes=4096),
                evidence=dict(peak_bytes=100, exit_code=None, docker_oom_killed=None,
                              attempt_before=None, attempt_after=None, parent_before=None,
                              parent_after=None, populated=False, terminal_snapshot=snapshot),
                plan=fixture["plan"], result=None, cleanup="complete")
            if version == 2:
                receipt["resource_model"] = "6" * 64
            else:
                receipt.update(envelope=fixture["plan"]["envelope_sha256"],
                               qualification_failure=None)
            with self.subTest(version=version, io="legacy omitted"):
                contract.validate("Receipt", receipt, version=version)
            # The per-field lexical bound is schema syntax; the shared byte sum
            # remains native semantic validation, including at this boundary.
            for raw in (None, "", "8:0 rbytes=1 cost.usage=9\n", "x" * 4096):
                snapshot["io_stat_raw"] = raw
                with self.subTest(version=version, io=repr(raw)[:60]):
                    wire = contract.parse_json(contract.canonical_bytes(receipt))
                    contract.validate("Receipt", wire, version=version)
                    contract.validate("Response", {"version": version,
                                                   "result": {"kind": "receipt", "receipt": wire}}, version=version)
            for raw in (0, False, {}, "x" * 4097, "\u00e9", "\t", "\r\n", "\x00", "\x7f"):
                snapshot["io_stat_raw"] = raw
                with self.subTest(version=version, invalid_io=repr(raw)[:60]):
                    with self.assertRaises(contract.ContractError):
                        contract.validate("Receipt", receipt, version=version)
            snapshot["io_stat_raw"] = None
            snapshot["io_stat"] = "8:0 rbytes=1\n"
            with self.assertRaises(contract.ContractError):
                contract.validate("Receipt", receipt, version=version)
            del snapshot["io_stat"]
            for required in ("memory_peak_raw", "unit_invocation"):
                invalid = copy.deepcopy(receipt)
                del invalid["evidence"]["terminal_snapshot"][required]
                with self.subTest(version=version, missing=required):
                    with self.assertRaises(contract.ContractError):
                        contract.validate("Receipt", invalid, version=version)

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

    def test_authenticated_qualified_grant_selects_version_three_without_fallback(self):
        value = qualified_grant()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "grant.json"
            for supplied, valid in ((value, True),
                                    ({**value, "version": 2}, False),
                                    ({**value, "kind": "film-measurement-grant"}, False),
                                    ({**value, "version": True}, False)):
                if path.exists():
                    path.chmod(0o644)
                data = contract.canonical_bytes(supplied)
                path.write_bytes(data)
                path.chmod(0o444)
                with patch.object(contract, "GRANT", path), \
                     patch.dict(os.environ, SLIPSTREAM_FILM_GRANT_SHA256=hashlib.sha256(data).hexdigest()):
                    if valid and os.getuid() == 0:
                        self.assertEqual(contract.read_grant(), value)
                    else:
                        with self.assertRaises(contract.ContractError):
                            contract.read_grant()


if __name__ == "__main__":
    unittest.main()
