"""Behavioral coverage for the deterministic spektrafilm-rs bundle manifest."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import tempfile
import types
import unittest
import unittest.mock
from contextlib import redirect_stdout
from pathlib import Path
FORK_COMMIT = "21f4788f42055fbc1b354acf00e0cbfeaa855af7"


def load(name: str) -> types.ModuleType:
    location = Path(__file__).with_name("film") / f"{name}.py"
    spec = importlib.util.spec_from_file_location(f"film_{name}", location)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


manifest = load("manifest")


class ManifestBundleTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        self.output_root = root / "slipstream-film"
        self.output_root.mkdir()
        self.binary = self.output_root / "spektrafilm-f64"
        self.binary.write_bytes(b"\x7fELF fixture\n")
        self.binary.chmod(0o755)
        self.data_root = self.output_root / "data"
        self.data_root.mkdir()
        (self.data_root / "profiles").mkdir()
        self.data_file = self.data_root / "profiles" / "portra.json"
        self.data_file.write_text('{"profile":"fixture"}\n')
        self.runtime_root = self.output_root / "lib"
        self.runtime_root.mkdir()
        (self.runtime_root / "libraw_r.so.25").write_bytes(b"native fixture\n")
        self.parameters = self.output_root / "parameters-default.json"
        self.parameters.write_text('{"camera":{},"scanner":{}}\n')

    def run_main(self) -> dict:
        with contextlib.ExitStack() as stack:
            for name, value in (
                ("OUTPUT", self.output_root),
                ("BINARY", self.binary),
                ("DATA_ROOT", self.data_root),
                ("RUNTIME_ROOT", self.runtime_root),
                ("DEFAULT_PARAMETERS", self.parameters),
            ):
                stack.enter_context(unittest.mock.patch.object(manifest, name, value))
            stack.enter_context(
                unittest.mock.patch.dict(os.environ, {"SPEKTRAFILM_FORK_COMMIT": FORK_COMMIT})
            )
            stdout = io.StringIO()
            with redirect_stdout(stdout):
                manifest.main()
        return json.loads(stdout.getvalue())

    def emitted_document(self) -> dict:
        return json.loads((self.output_root / "bundle-manifest.json").read_bytes())

    def test_bundle_is_the_digest_of_the_emitted_manifest_document(self):
        report = self.run_main()
        emitted = (self.output_root / "bundle-manifest.json").read_bytes()
        self.assertEqual(
            hashlib.sha256(emitted).hexdigest(),
            (self.output_root / "bundle").read_text().strip(),
        )
        self.assertEqual(hashlib.sha256(emitted).hexdigest(), report["bundle"])
        document = self.emitted_document()
        self.assertEqual(document["format"], 2)
        self.assertEqual(document["implementation"], "spektrafilm-rs")
        self.assertEqual(document["forkCommit"], FORK_COMMIT)
        self.assertEqual(document["binary"], str(self.binary))
        self.assertEqual(document["dataRoot"], str(self.data_root))
        self.assertEqual(document["files"][str(self.binary)], manifest.digest(self.binary))
        self.assertEqual(document["data"]["profiles/portra.json"], manifest.digest(self.data_file))

    def test_every_data_entry_resolves_inside_its_recorded_root(self):
        self.run_main()
        document = self.emitted_document()
        for name, digest in document["data"].items():
            resolved = Path(document["dataRoot"]) / name
            self.assertTrue(resolved.is_file())
            self.assertEqual(hashlib.sha256(resolved.read_bytes()).hexdigest(), digest)

    def test_missing_binary_refuses_the_bundle(self):
        self.binary.unlink()
        with self.assertRaises(SystemExit) as raised:
            self.run_main()
        self.assertIn("bundle inputs missing", str(raised.exception))

    def test_data_bytes_change_the_bundle_digest(self):
        first = self.run_main()
        self.data_file.write_text('{"profile":"changed"}\n')
        second = self.run_main()
        self.assertNotEqual(first["bundle"], second["bundle"])

    def test_unchanged_inputs_keep_the_bundle_digest(self):
        first, second = self.run_main()["bundle"], self.run_main()["bundle"]
        self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
