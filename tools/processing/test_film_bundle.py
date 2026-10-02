"""Behavioral coverage for the standalone Film bundle manifest helper.

``manifest.main`` must fold every listed bundle input — the runner, the
runtime-generated default parameter tree, the recorded processing-bundle
identity, the locked requirements, and the installed package list — into
the published bundle digest, must refuse any missing input, and must
publish a ``bundle`` that is exactly the SHA-256 of the emitted
``bundle-manifest.json`` bytes. Every tree entry must resolve, with the
digest the manifest records, inside the tree root the manifest itself
names for it: the server startup check joins each ``runtime``, ``source``,
and ``probe`` entry against those roots, so an entry the join cannot
resolve (a source tree listed against a parent of the recorded
``source_root``) leaves the Film stage unavailable. Everything runs
against disposable local file fixtures: no network, no engine, no
container.
"""

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

SPEKTRAFILM_COMMIT = "3bb2c2d2801ff68b92019cf1dbcbb133d60832bc"


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
        # The pinned layout the extended image carries: the patched source
        # tree behind the recorded source_root, the interpreter environment
        # behind the engine, the fixed-recipe probe modules, and the
        # bundle's own recorded assets.
        self.source_root = root / "spektrafilm" / "src"
        (self.source_root / "spektrafilm" / "utils").mkdir(parents=True)
        (self.source_root / "spektrafilm" / "sim.py").write_text("# pinned simulator\n")
        (self.source_root / "spektrafilm" / "utils" / "bounded_output.py").write_text(
            "# bounded output\n"
        )
        self.runtime_root = root / "runtime"
        (self.runtime_root / "bin").mkdir(parents=True)
        self.engine = self.runtime_root / "bin" / "python"
        self.engine.write_text("#!/bin/sh\nexit 0\n")
        (self.runtime_root / "lib").mkdir()
        (self.runtime_root / "lib" / "site-packages.py").write_text("# locked\n")
        self.probe_root = root / "probe"
        self.probe_root.mkdir()
        (self.probe_root / "film.py").write_text("# fixed recipe\n")
        self.output_root = root / "slipstream-film"
        self.runner = self.output_root / "runner" / "film_runner.py"
        self.runner.parent.mkdir(parents=True)
        self.runner.write_text("# local runner\n")
        self.parameters = self.output_root / "parameters-default.json"
        self.parameters.write_text('{"module": "spektrafilm"}\n')
        self.processing_bundle = root / "processing-bundle.json"
        self.processing_bundle.write_text('{"schema": 1}\n')
        self.requirements = root / "requirements.lock"
        self.requirements.write_text("# locked\n")
        self.packages = root / "os-packages.txt"
        self.packages.write_text("libgomp1 6-1\n")

    def run_main(self) -> dict:
        with contextlib.ExitStack() as stack:
            for name, value in (
                ("SOURCE_ROOT", self.source_root),
                ("RUNTIME_ROOT", self.runtime_root),
                ("PROBE_ROOT", self.probe_root),
                ("OUTPUT", self.output_root),
                ("RUNNER", self.runner),
                ("ENGINE", self.engine),
                ("DEFAULT_PARAMETERS", self.parameters),
                ("PROCESSING_BUNDLE", self.processing_bundle),
                ("REQUIREMENTS", self.requirements),
                ("PACKAGES", self.packages),
            ):
                stack.enter_context(unittest.mock.patch.object(manifest, name, value))
            stack.enter_context(
                unittest.mock.patch.dict(os.environ, {"SPEKTRAFILM_COMMIT": SPEKTRAFILM_COMMIT})
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
        self.assertEqual(document["format"], 1)
        self.assertEqual(document["spektrafilm_commit"], SPEKTRAFILM_COMMIT)
        self.assertEqual(document["engine"], str(self.engine))
        self.assertEqual(document["runner"], str(self.runner))
        self.assertEqual(document["source_root"], str(self.source_root))
        self.assertEqual(document["probe_root"], str(self.probe_root))
        self.assertEqual(
            document["files"][str(self.runner)],
            hashlib.sha256(self.runner.read_bytes()).hexdigest(),
        )

    def test_every_tree_entry_resolves_inside_its_recorded_root(self):
        self.run_main()
        document = self.emitted_document()
        for tree, tree_root in (
            ("runtime", Path(document["engine"]).parent.parent),
            ("source", Path(document["source_root"])),
            ("probe", Path(document["probe_root"])),
        ):
            entries = document[tree]
            self.assertTrue(entries)
            for name, digest in entries.items():
                resolved = tree_root / name
                self.assertTrue(
                    resolved.is_file(),
                    f"{tree} entry {name} must resolve under {tree_root}",
                )
                self.assertEqual(hashlib.sha256(resolved.read_bytes()).hexdigest(), digest)

    def test_missing_runner_refuses_the_bundle(self):
        self.runner.unlink()
        with self.assertRaises(SystemExit) as raised:
            self.run_main()
        self.assertIn("bundle inputs missing", str(raised.exception))

    def test_source_bytes_change_the_bundle_digest(self):
        first = self.run_main()
        (self.source_root / "spektrafilm" / "sim.py").write_text("# patched simulator\n")
        second = self.run_main()
        self.assertNotEqual(first["bundle"], second["bundle"])

    def test_unchanged_inputs_keep_the_bundle_digest(self):
        first, second = self.run_main()["bundle"], self.run_main()["bundle"]
        self.assertEqual(first, second)
        self.assertEqual((self.output_root / "bundle").read_text().strip(), first)


if __name__ == "__main__":
    unittest.main()
