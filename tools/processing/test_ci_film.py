"""Isolated Film executable-cache behavior; native cold/hit smoke is separate."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "build-spektrafilm.py"
SPEC = importlib.util.spec_from_file_location("build_spektrafilm", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
film = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(film)

OUTPUT = Path("third_party/spektrafilm-rs/target/release/spektrafilm-f64")
EXECUTABLE = b"#!/bin/sh\nprintf 'isolated Film fixture\\n'\n"
COMMIT = "a" * 40


class FilmArtifactTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name) / "checkout"
        self.root.mkdir()
        self.artifact = Path(directory.name) / "artifact"
        self.output = self.root / OUTPUT
        self.inputs = {
            "source": {"commit": COMMIT, "files": {"src/main.rs": "source-digest"}},
            "toolchain": {"rustc": "rustc fixture"},
            "native": {"image": "image fixture", "libraw": "native-digest"},
            "policy": {"features": ["cli", "f64"], "profile": "release"},
        }
        self.libraries = {"/fixture/libraw.so": "b" * 64}
        self.build_inputs = self.start_patch("build_inputs", side_effect=lambda root: copy.deepcopy(self.inputs))
        self.cargo = self.start_patch("run_cargo", side_effect=self.build_fixture)
        self.runtime = self.start_patch("runtime_libraries", return_value=self.libraries)
        self.describe = self.start_patch("describe")

    def start_patch(self, name: str, **kwargs):
        patcher = patch.object(film, name, **kwargs)
        self.addCleanup(patcher.stop)
        return patcher.start()

    def build_fixture(self, root: Path, commit: str | None = None) -> None:
        self.assertEqual(root, self.root)
        self.assertFalse(self.artifact.exists(), "an incomplete build must not publish the cache directory")
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.output.write_bytes(EXECUTABLE)
        self.output.chmod(0o755)

    def prepare(self) -> None:
        self.assertIsNone(film.prepare(self.root, self.artifact))

    def seal(self) -> None:
        self.prepare()
        self.output.unlink()
        for mock in (self.build_inputs, self.cargo, self.runtime, self.describe):
            mock.reset_mock()

    def manifest(self) -> dict:
        return json.loads((self.artifact / "manifest.json").read_bytes())

    def assert_refused_without_publication(self) -> None:
        with self.assertRaises((ValueError, RuntimeError, OSError)):
            self.prepare()
        self.cargo.assert_not_called()
        self.assertFalse(self.output.exists(), "refused cache must not publish a local executable")

    def test_cold_build_seals_only_executable_and_full_manifest(self):
        self.prepare()
        self.cargo.assert_called_once_with(self.root, COMMIT)
        self.assertEqual(
            {path.name for path in self.artifact.iterdir()},
            {"spektrafilm-f64", "manifest.json"},
        )
        cached = self.artifact / "spektrafilm-f64"
        self.assertEqual(cached.read_bytes(), EXECUTABLE)
        self.assertFalse(cached.is_symlink())
        self.assertTrue(os.access(cached, os.X_OK))
        self.assertEqual(
            self.manifest(),
            {
                "format": 1,
                "inputs": self.inputs,
                "binarySha256": hashlib.sha256(EXECUTABLE).hexdigest(),
                "libraries": self.libraries,
            },
        )
        self.assertEqual(self.output.read_bytes(), EXECUTABLE)
        self.assertTrue(os.access(self.output, os.X_OK))
        self.runtime.assert_called()
        self.describe.assert_called()
        self.assertEqual(self.describe.call_args.args[1], COMMIT)

    def test_restored_cache_replaces_local_output_without_cargo(self):
        self.seal()
        self.output.write_bytes(b"unrelated previous executable")
        manifest_bytes = (self.artifact / "manifest.json").read_bytes()
        self.prepare()
        self.cargo.assert_not_called()
        self.assertEqual(self.output.read_bytes(), EXECUTABLE)
        self.assertFalse(self.output.is_symlink())
        self.assertTrue(os.access(self.output, os.X_OK))
        self.assertEqual((self.artifact / "manifest.json").read_bytes(), manifest_bytes)
        self.runtime.assert_called()
        self.describe.assert_called()
        self.assertEqual(self.describe.call_args.args[1], COMMIT)

    def test_changed_identity_is_refused_before_runtime_validation(self):
        self.seal()
        original = copy.deepcopy(self.inputs)
        self.runtime.side_effect = AssertionError("stale identity must be rejected before library inspection")
        self.describe.side_effect = AssertionError("stale identity must be rejected before introspection")
        for component in ("source", "toolchain", "native", "policy"):
            with self.subTest(component=component):
                self.inputs = copy.deepcopy(original)
                self.inputs[component]["changed"] = "different exact build input"
                self.assert_refused_without_publication()
        self.runtime.assert_not_called()
        self.describe.assert_not_called()

    def test_missing_manifest_is_not_a_cold_build(self):
        self.seal()
        (self.artifact / "manifest.json").unlink()
        self.assert_refused_without_publication()

    def test_invalid_manifest_is_never_rebuilt(self):
        self.seal()
        valid = self.manifest()
        cases = {
            "empty": b"",
            "corrupt-json": b"{not json}",
            "not-an-object": b"[]",
            "incomplete": json.dumps({"format": 1, "inputs": self.inputs}).encode(),
            "unsupported-format": json.dumps({**valid, "format": 2}).encode(),
        }
        for name, content in cases.items():
            with self.subTest(manifest=name):
                (self.artifact / "manifest.json").write_bytes(content)
                self.assert_refused_without_publication()

    def test_existing_empty_directory_is_refused_not_rebuilt(self):
        self.artifact.mkdir()
        self.assert_refused_without_publication()

    def test_modified_binary_is_refused_before_publication(self):
        self.seal()
        (self.artifact / "spektrafilm-f64").write_bytes(EXECUTABLE + b"# modified\n")
        self.assert_refused_without_publication()

    def test_missing_binary_is_refused_before_publication(self):
        self.seal()
        (self.artifact / "spektrafilm-f64").unlink()
        self.assert_refused_without_publication()

    def test_nonexecutable_binary_is_refused_before_publication(self):
        self.seal()
        (self.artifact / "spektrafilm-f64").chmod(0o644)
        self.assert_refused_without_publication()

    def test_symlink_binary_is_refused_even_when_target_hash_matches(self):
        self.seal()
        external = self.root / "external-executable"
        external.write_bytes(EXECUTABLE)
        external.chmod(0o755)
        cached = self.artifact / "spektrafilm-f64"
        cached.unlink()
        cached.symlink_to(external)
        self.assert_refused_without_publication()
        self.assertEqual(external.read_bytes(), EXECUTABLE)

    def test_changed_runtime_library_map_is_refused_without_rebuild(self):
        self.seal()
        self.runtime.return_value = {"/fixture/libraw.so": "c" * 64}
        self.assert_refused_without_publication()
        self.runtime.assert_called()

    def test_failed_introspection_preserves_existing_output(self):
        self.seal()
        previous = b"previous local executable"
        self.output.write_bytes(previous)
        self.describe.side_effect = ValueError("embedded fork commit does not match")
        with self.assertRaisesRegex(ValueError, "embedded fork commit"):
            self.prepare()
        self.cargo.assert_not_called()
        self.assertEqual(self.output.read_bytes(), previous)

    def test_failed_cold_build_never_creates_artifact(self):
        failure = subprocess.CalledProcessError(17, ["cargo", "build"])

        def fail_after_partial_build(root: Path, commit: str | None = None) -> None:
            self.build_fixture(root, commit)
            raise failure

        self.cargo.side_effect = fail_after_partial_build
        with self.assertRaises(subprocess.CalledProcessError) as raised:
            self.prepare()
        self.assertIs(raised.exception, failure)
        self.assertFalse(self.artifact.exists())
        self.runtime.assert_not_called()
        self.describe.assert_not_called()

    def test_failed_cold_validation_does_not_seal_build(self):
        self.describe.side_effect = ValueError("invalid Film introspection")
        with self.assertRaisesRegex(ValueError, "invalid Film introspection"):
            self.prepare()
        self.cargo.assert_called_once()
        self.assertFalse(self.artifact.exists())


class FilmArtifactPolicyTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.base = Path(directory.name)
        self.root = self.base / "checkout"
        self.source = self.root / "third_party" / "spektrafilm-rs"
        self.source.mkdir(parents=True)
        self.home = self.base / "home"
        self.home.mkdir()
        self.cargo_home = self.base / "isolated-cargo-home"
        self.cargo_home.mkdir()
        environment = patch.dict(
            os.environ,
            {"HOME": str(self.home), "CARGO_HOME": str(self.cargo_home)},
            clear=True,
        )
        environment.start()
        self.addCleanup(environment.stop)
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)

    def test_allows_non_artifact_build_scheduling_and_dev_test_settings(self):
        with patch.dict(os.environ, {
            "CARGO_BUILD_JOBS": "2",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_TEST_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
        }):
            film.artifact_policy(self.root)

    def test_rejects_cargo_config_in_each_build_cwd_ancestor(self):
        # Cargo searches the invocation directory and ancestors, including outside checkout.
        for ancestor in (self.root, self.base):
            for name in ("config", "config.toml"):
                with self.subTest(ancestor=ancestor.relative_to(self.base), name=name):
                    config = ancestor / ".cargo" / name
                    config.parent.mkdir(exist_ok=True)
                    config.write_text('[build]\njobs = 2\n')
                    try:
                        with self.assertRaises((ValueError, RuntimeError)):
                            film.artifact_policy(self.root)
                    finally:
                        config.unlink()

    def test_rejects_config_in_selected_cargo_home(self):
        for name in ("config", "config.toml"):
            with self.subTest(name=name):
                config = self.cargo_home / name
                config.write_text('[build]\njobs = 2\n')
                try:
                    with self.assertRaises((ValueError, RuntimeError)):
                        film.artifact_policy(self.root)
                finally:
                    config.unlink()

    def test_explicit_cargo_home_does_not_read_unused_default_home(self):
        default_home = self.home / ".cargo"
        default_home.mkdir()
        (default_home / "config.toml").write_text('[build]\nrustflags = ["-Ctarget-cpu=native"]\n')
        film.artifact_policy(self.root)

    def test_rejects_native_compiler_and_flag_overrides(self):
        overrides = {
            "CC": "clang",
            "CXX": "clang++",
            "AR": "llvm-ar",
            "CC_x86_64_unknown_linux_gnu": "clang",
            "CXX_x86_64_unknown_linux_gnu": "clang++",
            "AR_x86_64_unknown_linux_gnu": "llvm-ar",
            "CFLAGS": "-march=native",
            "CFLAGS_x86_64_unknown_linux_gnu": "-O3",
            "CXXFLAGS": "-march=native",
            "CPPFLAGS": "-DFIXTURE",
            "LDFLAGS": "-L/fixture",
            "HOST_CC": "clang",
            "TARGET_CXX": "clang++",
        }
        self.assert_overrides_refused(overrides)

    def test_rejects_rust_compiler_flags_target_and_release_policy_overrides(self):
        overrides = {
            "RUSTC": "/fixture/rustc",
            "RUSTDOC": "/fixture/rustdoc",
            "RUSTFLAGS": "-Ctarget-cpu=native",
            "RUSTDOCFLAGS": "--cfg fixture",
            "RUSTC_WRAPPER": "/fixture/wrapper",
            "RUSTC_WORKSPACE_WRAPPER": "/fixture/wrapper",
            "CARGO_ENCODED_RUSTFLAGS": "-C\x1ftarget-cpu=native",
            "CARGO_ENCODED_RUSTDOCFLAGS": "--cfg\x1ffixture",
            "CARGO_TARGET_DIR": "/fixture/target",
            "CARGO_BUILD_TARGET": "x86_64-unknown-linux-gnu",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "clang",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "-Ctarget-cpu=native",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": "1",
            "CARGO_PROFILE_RELEASE_DEBUG": "1",
            "CARGO_BUILD_RUSTFLAGS": "-Ctarget-cpu=native",
            "CARGO_BUILD_RUSTC": "/fixture/rustc",
            "CARGO_BUILD_RUSTDOC": "/fixture/rustdoc",
            "CARGO_BUILD_TARGET_DIR": "/fixture/target",
        }
        self.assert_overrides_refused(overrides)

    def assert_overrides_refused(self, overrides: dict[str, str]) -> None:
        for name, value in overrides.items():
            with self.subTest(variable=name), patch.dict(os.environ, {name: value}):
                with self.assertRaises((ValueError, RuntimeError)):
                    film.artifact_policy(self.root)


class FilmSourceInputsTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / "third_party" / "spektrafilm-rs"
        self.source.mkdir(parents=True)
        self.git("init", "--quiet")
        (self.source / ".gitignore").write_text("/target/\n")
        (self.source / "src").mkdir()
        self.main = self.source / "src" / "main.rs"
        self.main.write_bytes(b"fn main() {}\n")
        self.main.chmod(0o644)
        self.git("add", ".")
        self.git("-c", "user.name=Film fixture", "-c", "user.email=film@example.invalid", "commit", "--quiet", "-m", "fixture")

    def git(self, *arguments: str) -> str:
        result = subprocess.run(
            ["git", *arguments],
            cwd=self.source,
            env={**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull},
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()

    def inputs(self) -> dict:
        return film.source_inputs(self.root)

    @staticmethod
    def key(inputs: dict) -> str:
        return hashlib.sha256(json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

    def test_actual_tracked_bytes_change_identity_without_changing_commit(self):
        first = self.inputs()
        self.assertEqual(first["commit"], self.git("rev-parse", "HEAD"))
        self.assertEqual(
            first["files"]["src/main.rs"]["sha256"],
            hashlib.sha256(self.main.read_bytes()).hexdigest(),
        )
        self.assertEqual(self.inputs(), first)
        self.main.write_bytes(b"fn main() { println!(\"changed\"); }\n")
        changed = self.inputs()
        self.assertEqual(changed["commit"], first["commit"])
        self.assertNotEqual(self.key(changed), self.key(first))
        self.assertEqual(
            changed["files"]["src/main.rs"]["sha256"],
            hashlib.sha256(self.main.read_bytes()).hexdigest(),
        )

    def test_untracked_source_is_hashed_but_ignored_build_output_is_not(self):
        first = self.inputs()
        target = self.source / "target" / "release"
        target.mkdir(parents=True)
        (target / "spektrafilm-f64").write_bytes(b"ignored build output")
        self.assertEqual(self.inputs(), first)
        untracked = self.source / "src" / "new.rs"
        untracked.write_bytes(b"pub const VALUE: u8 = 1;\n")
        changed = self.inputs()
        self.assertEqual(
            changed["files"]["src/new.rs"]["sha256"],
            hashlib.sha256(untracked.read_bytes()).hexdigest(),
        )
        self.assertNotEqual(self.key(changed), self.key(first))
        untracked.write_bytes(b"pub const VALUE: u8 = 2;\n")
        self.assertNotEqual(self.key(self.inputs()), self.key(changed))

    def test_executable_mode_changes_identity_without_changing_bytes(self):
        first = self.inputs()
        self.main.chmod(0o755)
        changed = self.inputs()
        self.assertEqual(
            changed["files"]["src/main.rs"]["sha256"],
            first["files"]["src/main.rs"]["sha256"],
        )
        self.assertNotEqual(
            changed["files"]["src/main.rs"]["mode"],
            first["files"]["src/main.rs"]["mode"],
        )
        self.assertNotEqual(self.key(changed), self.key(first))

    def test_missing_tracked_source_remains_part_of_identity(self):
        first = self.inputs()
        self.main.unlink()
        changed = self.inputs()
        self.assertEqual(changed["files"]["src/main.rs"], {"missing": True})
        self.assertNotEqual(self.key(changed), self.key(first))


if __name__ == "__main__":
    unittest.main()
