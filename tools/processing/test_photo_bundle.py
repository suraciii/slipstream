"""Behavioral coverage for the native photo bundle build helpers.

``build.git_revision`` must pin the exact committed native source: loose
bytes the image build context would receive -- an unstaged change to a
tracked file (in the superproject or inside an initialized submodule), an
untracked file, an ignored build-tree artifact, or a broken submodule --
fail the build, while bytes the native ``.dockerignore`` excludes
(``.git`` and ``docker-images``) may sit in the worktree without
rejecting it. ``build.verify_pinned_icc`` must accept the repository's
qualified ICC asset and refuse any different bytes. ``manifest.main``
must fold every listed bundle input, including the os-packages lock and
the native engine tree, into the published bundle digest, must refuse a
missing engine binary, and must publish a ``bundle`` that is exactly the
SHA-256 of the emitted ``bundle-manifest.json`` bytes.

Everything runs against disposable local Git repositories and file
fixtures: no network, no engine, no container. The Git author is
configured through ``GIT_CONFIG_*`` environment variables scoped to the
spawned processes, and user/system Git configuration is ignored, so host
state cannot skew the results.
"""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import types
import unittest
import unittest.mock
from contextlib import redirect_stdout
from pathlib import Path

DARKTABLE_COMMIT = "0123456789abcdef0123456789abcdef01234567"

GIT_ENVIRONMENT = {
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_SYSTEM": "/dev/null",
    "GIT_CONFIG_COUNT": "3",
    "GIT_CONFIG_KEY_0": "user.name",
    "GIT_CONFIG_VALUE_0": "Slipstream Photo Bundle Tests",
    "GIT_CONFIG_KEY_1": "user.email",
    "GIT_CONFIG_VALUE_1": "photo-bundle-tests@slipstream.invalid",
    "GIT_CONFIG_KEY_2": "protocol.file.allow",
    "GIT_CONFIG_VALUE_2": "always",
}


def load(name: str) -> types.ModuleType:
    location = Path(__file__).with_name("photo") / f"{name}.py"
    spec = importlib.util.spec_from_file_location(f"photo_{name}", location)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


build = load("build")
manifest = load("manifest")
discover = load("discover")


def git(repository: Path, *arguments: str, stdin: str | None = None) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repository), *arguments],
        input=stdin, text=True, env={**os.environ, **GIT_ENVIRONMENT},
        stderr=subprocess.DEVNULL,
    )


def commit(repository: Path, message: str) -> None:
    git(repository, "add", "-A")
    git(repository, "commit", "-m", message)


def make_source(root: Path) -> tuple[Path, Path]:
    """A native-source stand-in: one superproject with one initialized submodule."""
    library = root / "library"
    library.mkdir()
    git(library, "init", "--initial-branch=main")
    (library / "engine.c").write_text("int engine(void) { return 1; }\n")
    commit(library, "library: base")

    source = root / "source"
    source.mkdir()
    git(source, "init", "--initial-branch=main")
    (source / ".gitignore").write_text("build/\n")
    (source / "CMakeLists.txt").write_text("cmake_minimum_required(VERSION 3.20)\n")
    git(source, "submodule", "add", "../library", "library")
    commit(source, "source: base")
    return source, library


def make_bundle_inputs(root: Path) -> tuple[Path, ...]:
    """Bundle inputs in the production role order (metadata, ICC, commit, packages)."""
    definitions = (
        ("engine-metadata.json", b'{"darktable_commit": "fixture"}\n'),
        ("LargeRGB-elle-V2-g10.icc", b"icc fixture bytes\n"),
        ("darktable-commit", b"0123456789abcdef0123456789abcdef01234567\n"),
        ("os-packages.txt", b"libimage/exact 1:2.3-4\n"),
    )
    files = []
    for name, payload in definitions:
        path = root / name
        path.write_bytes(payload)
        files.append(path)
    return tuple(files)


class ModuleMetadataTest(unittest.TestCase):
    def test_normalizes_list_and_object_module_payloads(self):
        entries = [{"operation": "exposure"}, {"operation": "rgbcurve"}]
        self.assertEqual(discover.normalize_module_entries(entries), entries)
        self.assertEqual(
            discover.normalize_module_entries({"modules": entries}),
            entries,
        )

    def test_rejects_malformed_or_duplicate_module_entries(self):
        with self.assertRaises(RuntimeError):
            discover.normalize_module_entries([{"operation": ""}])
        with self.assertRaises(RuntimeError):
            discover.normalize_module_entries(
                [{"operation": "exposure"}, {"operation": "exposure"}],
            )


class GitRevisionTest(unittest.TestCase):
    def setUp(self) -> None:
        environment = unittest.mock.patch.dict(os.environ, GIT_ENVIRONMENT)
        environment.start()
        self.addCleanup(environment.stop)
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.source, self.library = make_source(Path(directory.name))

    def revision(self, repository: Path) -> str:
        return git(repository, "rev-parse", "HEAD").strip()

    def test_clean_source_with_initialized_submodule_pins_head(self):
        self.assertEqual(build.git_revision(self.source), self.revision(self.source))

    def test_unstaged_change_to_a_tracked_file_is_rejected(self):
        (self.source / "CMakeLists.txt").write_text("cmake_minimum_required(VERSION 3.21)\n")
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("CMakeLists.txt", str(raised.exception))

    def test_unstaged_change_inside_the_submodule_is_rejected(self):
        (self.source / "library" / "engine.c").write_text("int engine(void) { return 2; }\n")
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("engine.c", str(raised.exception))

    def test_untracked_file_is_rejected(self):
        (self.source / "untracked.txt").write_text("loose bytes\n")
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("untracked.txt", str(raised.exception))

    def test_ignored_build_tree_bytes_are_rejected(self):
        build_tree = self.source / "build"
        build_tree.mkdir()
        (build_tree / "engine.o").write_bytes(b"object bytes\n")
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("engine.o", str(raised.exception))

    def test_uninitialized_submodule_is_rejected(self):
        git(self.source, "submodule", "deinit", "library")
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("submodule", str(raised.exception))

    def test_unresolved_submodule_is_rejected(self):
        git(self.library, "commit", "--allow-empty", "-m", "library: diverged")
        recorded = git(self.source, "ls-tree", "HEAD", "library").split()[2]
        moved = self.revision(self.library)
        entries = f"160000 {recorded} 2\tlibrary\n160000 {moved} 3\tlibrary\n"
        git(self.source, "update-index", "--index-info", stdin=entries)
        with self.assertRaises(SystemExit) as raised:
            build.git_revision(self.source)
        self.assertIn("library", str(raised.exception))

    def test_bytes_excluded_from_the_docker_context_do_not_reject(self):
        excluded = self.source / "docker-images"
        excluded.mkdir()
        (excluded / "retained-parent.tar").write_bytes(b"bytes the build context never receives\n")
        self.assertEqual(build.git_revision(self.source), self.revision(self.source))


class PinnedIccTest(unittest.TestCase):
    def test_accepts_the_repository_asset_and_refuses_different_bytes(self):
        # The repository carries the qualified profile itself.
        build.verify_pinned_icc()

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            icc_directory = root / "tools" / "processing" / "photo" / "icc"
            icc_directory.mkdir(parents=True)
            (icc_directory / "LargeRGB-elle-V2-g10.icc").write_bytes(b"not the qualified profile\n")
            with unittest.mock.patch.object(build, "ROOT", root):
                with self.assertRaises(SystemExit) as raised:
                    build.verify_pinned_icc()
            self.assertIn("does not match the qualified profile", str(raised.exception))

            (icc_directory / "LargeRGB-elle-V2-g10.icc").unlink()
            with unittest.mock.patch.object(build, "ROOT", root):
                with self.assertRaises(SystemExit) as raised:
                    build.verify_pinned_icc()
            self.assertIn("missing from the repository", str(raised.exception))


class ManifestBundleTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        self.native_root = root / "native"
        (self.native_root / "bin").mkdir(parents=True)
        self.engine = self.native_root / "bin" / "darktable-mcp"
        self.engine.write_text("#!/bin/sh\nexit 0\n")
        (self.native_root / "lib").mkdir()
        (self.native_root / "lib" / "libdarktable.so").write_bytes(b"elf fixture\n")
        self.files = make_bundle_inputs(root)
        self.output_root = root / "output"
        self.output_root.mkdir()

    def run_main(self) -> dict:
        with contextlib.ExitStack() as stack:
            stack.enter_context(unittest.mock.patch.object(manifest, "NATIVE_ROOT", self.native_root))
            stack.enter_context(unittest.mock.patch.object(manifest, "ENGINE", self.engine))
            stack.enter_context(unittest.mock.patch.object(manifest, "FILES", self.files))
            stack.enter_context(unittest.mock.patch.object(manifest, "METADATA", self.files[0]))
            stack.enter_context(unittest.mock.patch.object(manifest, "ICC", self.files[1]))
            stack.enter_context(unittest.mock.patch.object(manifest, "COMMIT", self.files[2]))
            stack.enter_context(unittest.mock.patch.object(manifest, "PACKAGES", self.files[3]))
            stack.enter_context(unittest.mock.patch.object(manifest, "OUTPUT", self.output_root))
            stack.enter_context(unittest.mock.patch.dict(os.environ, {"DARKTABLE_COMMIT": DARKTABLE_COMMIT}))
            stdout = io.StringIO()
            with redirect_stdout(stdout):
                manifest.main()
        return json.loads(stdout.getvalue())

    def test_bundle_is_the_digest_of_the_emitted_manifest_document(self):
        report = self.run_main()
        emitted = (self.output_root / "bundle-manifest.json").read_bytes()
        self.assertEqual(
            hashlib.sha256(emitted).hexdigest(),
            (self.output_root / "bundle").read_text().strip(),
        )
        document = json.loads(emitted)
        self.assertEqual(document, {key: value for key, value in report.items() if key != "bundle"})
        self.assertEqual(document["format"], 1)
        self.assertEqual(document["darktable_commit"], DARKTABLE_COMMIT)
        self.assertEqual(document["engine"], str(self.engine))
        self.assertIn("bin/darktable-mcp", document["native"])
        self.assertNotIn("film_parent_manifest", document)

    def test_os_packages_bytes_change_the_bundle_digest(self):
        first = self.run_main()
        self.files[-1].write_bytes(self.files[-1].read_bytes() + b"libimage-extra 1:2.3-4\n")
        second = self.run_main()
        self.assertNotEqual(first["bundle"], second["bundle"])
        self.assertNotEqual(
            first["files"][str(self.files[-1])],
            second["files"][str(self.files[-1])],
        )

    def test_native_engine_bytes_change_the_bundle_digest(self):
        first = self.run_main()
        self.engine.write_text("#!/bin/sh\nexit 3\n")
        second = self.run_main()
        self.assertNotEqual(first["bundle"], second["bundle"])
        self.assertNotEqual(
            first["native"]["bin/darktable-mcp"],
            second["native"]["bin/darktable-mcp"],
        )

    def test_missing_engine_binary_refuses_the_bundle(self):
        self.engine.unlink()
        with self.assertRaises(SystemExit) as raised:
            self.run_main()
        self.assertIn("bundle inputs missing", str(raised.exception))

    def test_unchanged_inputs_keep_the_bundle_digest(self):
        first, second = self.run_main()["bundle"], self.run_main()["bundle"]
        self.assertEqual(first, second)
        self.assertEqual((self.output_root / "bundle").read_text().strip(), first)


if __name__ == "__main__":
    unittest.main()
