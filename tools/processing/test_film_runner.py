"""Consumer-facing destination refusal regressions for the Film runner.

The service reserves output with an empty regular file. The runner must admit
that reservation while preserving nonempty files and refusing symlinks and
other destination types before importing the numerical engine. PNG encoding
and seeded pixel determinism are exercised with the real packaged runtime.
"""

from __future__ import annotations

from copy import deepcopy
import importlib.util
import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


location = Path(__file__).with_name("film") / "film_runner.py"
spec = importlib.util.spec_from_file_location("film_runner", location)
runner = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(runner)


class OutputDestinationGateTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.input_path = self.root / "development.tiff"
        self.input_path.write_bytes(b"gate fixture; never decoded")
        self.parameters = self.root / "parameters.json"
        self.parameters.write_text("{}")
        # Stop accepted destinations at the next real CLI gate, before
        # native imports, independently of the test host's environment.
        environment = patch.dict(os.environ, {}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)

    def render(self, output_path):
        errors = io.StringIO()
        with redirect_stderr(errors):
            code = runner.main([
                "preview",
                "--input", str(self.input_path),
                "--output", str(output_path),
                "--parameters", str(self.parameters),
                "--max-edge", "64",
            ])
        self.assertEqual(code, runner.REFUSED, errors.getvalue())
        return errors.getvalue()

    def assert_admitted(self, output_path):
        errors = self.render(output_path)
        self.assertIn("outside the pinned deterministic engine environment", errors)
        self.assertNotIn("sealed development source", errors)

    def assert_refused(self, output_path):
        errors = self.render(output_path)
        self.assertIn("without the sealed development source", errors)
        self.assertNotIn("pinned deterministic engine environment", errors)

    def test_missing_destination_is_admitted(self):
        self.assert_admitted(self.root / "attempt" / "preview.tiff")

    def test_service_reserved_empty_regular_file_is_admitted(self):
        reserved = self.root / "preview.tiff"
        reserved.write_bytes(b"")
        self.assert_admitted(reserved)
        self.assertEqual(reserved.read_bytes(), b"")

    def test_nonempty_user_file_is_refused_and_preserved(self):
        user_file = self.root / "preview.tiff"
        user_file.write_bytes(b"user data")
        self.assert_refused(user_file)
        self.assertEqual(user_file.read_bytes(), b"user data")

    def test_symlink_to_empty_file_is_refused_and_preserved(self):
        target = self.root / "reserved"
        target.write_bytes(b"")
        symlink = self.root / "preview.tiff"
        symlink.symlink_to(target)
        self.assert_refused(symlink)
        self.assertTrue(symlink.is_symlink())
        self.assertEqual(target.read_bytes(), b"")

    def test_dangling_symlink_is_refused(self):
        symlink = self.root / "preview.tiff"
        symlink.symlink_to(self.root / "absent")
        self.assert_refused(symlink)
        self.assertTrue(symlink.is_symlink())

    def test_directory_destination_is_refused(self):
        directory = self.root / "a-directory"
        directory.mkdir()
        self.assert_refused(directory)
        self.assertTrue(directory.is_dir())

    def test_fifo_destination_is_refused(self):
        fifo = self.root / "preview.tiff"
        os.mkfifo(fifo)
        self.assert_refused(fifo)


class FixedRecipeIdentityTest(unittest.TestCase):
    def setUp(self):
        tree_path = (
            Path(__file__).resolve().parents[2]
            / "crates/slipstream-processing/src/spektrafilm-recipe-tree.json"
        )
        tree = json.loads(tree_path.read_text())
        names = {"filmRender": "film_render", "printRender": "print_render"}
        self.recipe = {
            names.get(key, key): value for key, value in tree.items()
            if key != "output"
        }
        self.recipe.update(film="kodak_portra_400", paper="kodak_portra_endura", seed=327)
        self.recipe["processing_bundle"] = {"files_sha256": {"Dockerfile": "a" * 64}}
        self.recipe["gamut_workspace_allowance_bytes"] = 128 * 1024 * 1024
        engine = patch.dict("sys.modules", {"film": SimpleNamespace(SEED=327)})
        engine.start()
        self.addCleanup(engine.stop)

    def test_fixed_numerical_recipe_accepts_independent_build_and_workspace_identity(self):
        runner.check_recipe(self.recipe)
        changed = deepcopy(self.recipe)
        changed["processing_bundle"] = {"files_sha256": {"Dockerfile": "b" * 64}}
        changed["gamut_workspace_allowance_bytes"] *= 2
        runner.check_recipe(changed)

    def test_changed_numerical_parameters_stocks_and_seed_are_refused(self):
        for group, field, value in (
            ("camera", "exposure_compensation_ev", 0.5),
            ("film_render", "grain", {"active": False}),
            ("settings", "lut_resolution", 17),
            (None, "film", "kodak_gold_200"),
            (None, "paper", "other_paper"),
            (None, "seed", 328),
        ):
            with self.subTest(group=group, field=field):
                changed = deepcopy(self.recipe)
                (changed[group] if group else changed)[field] = value
                with self.assertRaises(runner.Refusal):
                    runner.check_recipe(changed)

    def test_missing_or_unknown_recipe_fields_are_refused(self):
        missing = deepcopy(self.recipe)
        del missing["taps"]
        unknown = deepcopy(self.recipe)
        unknown["unknown_numerical_control"] = True
        for changed in (missing, unknown):
            with self.assertRaises(runner.Refusal):
                runner.check_recipe(changed)


if __name__ == "__main__":
    unittest.main()
