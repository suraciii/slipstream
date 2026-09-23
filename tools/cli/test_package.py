"""Candidate archive contract without compiling or contacting a service."""

import hashlib
import importlib.util
import io
import tempfile
import tarfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "package-cli.py"
spec = importlib.util.spec_from_file_location("package_cli", SCRIPT)
package_cli = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_cli)


class CandidateArchiveTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="slipstream-cli-package-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        binary = self.root / "target" / "release" / "slipstream"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"synthetic executable")
        for source in package_cli.FILES:
            path = self.root / source
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="ascii")
        self.root_patch = patch.object(package_cli, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def tool_output(self, *args):
        if args[:2] == ("git", "status"):
            return ""
        if args[:2] == ("git", "rev-parse"):
            return "a" * 40
        if args[:2] == ("cargo", "metadata"):
            return '{"packages":[{"name":"slipstream-cli","version":"0.0.0"}]}'
        if args[:2] == ("cargo", "build"):
            return ""
        if args[1:] == ("--version",):
            return "slipstream 0.0.0"
        raise AssertionError(args)

    def test_archive_is_source_bound_bounded_and_never_replaces_an_artifact(self):
        with patch.object(package_cli, "run", side_effect=self.tool_output), redirect_stdout(io.StringIO()):
            package_cli.main()
            with self.assertRaises(FileExistsError):
                package_cli.main()
        name = "slipstream-cli-0.0.0-g" + "a" * 12 + "-linux-amd64"
        output = self.root / "dist" / name
        archive = output / f"{name}.tar.gz"
        self.assertEqual(archive.read_bytes()[4:8], bytes(4))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(
            (output / f"{name}.tar.gz.sha256").read_text(encoding="ascii"),
            f"{digest}  {archive.name}\n",
        )
        with tarfile.open(archive, "r:gz") as tar:
            expected = [f"{name}/slipstream", *[f"{name}/{item}" for item in package_cli.FILES.values()]]
            self.assertEqual(tar.getnames(), expected)
            for member in tar.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual((member.mtime, member.uid, member.gid), (0, 0, 0))
                self.assertEqual(member.mode, 0o755 if member.name.endswith("/slipstream") else 0o644)

    def test_platform_and_commit_guards_run_before_creating_output(self):
        with patch.object(package_cli.platform, "system", return_value="Darwin"):
            with self.assertRaisesRegex(ValueError, "Linux amd64"):
                package_cli.main()

        def invalid_commit(*args):
            if args[:2] == ("git", "rev-parse"):
                return "g" * 40
            return self.tool_output(*args)
        with patch.object(package_cli, "run", side_effect=invalid_commit):
            with self.assertRaisesRegex(ValueError, "full Git commit"):
                package_cli.main()
        self.assertFalse((self.root / "dist").exists())

    def test_binary_version_and_symlinked_output_are_refused(self):
        def wrong_version(*args):
            if args[1:] == ("--version",):
                return "slipstream 9.9.9"
            return self.tool_output(*args)
        with patch.object(package_cli, "run", side_effect=wrong_version):
            with self.assertRaisesRegex(ValueError, "does not match Cargo metadata"):
                package_cli.main()
        self.assertFalse((self.root / "dist").exists())
        target = self.root / "elsewhere"
        target.mkdir()
        (self.root / "dist").symlink_to(target, target_is_directory=True)
        with patch.object(package_cli, "run", side_effect=self.tool_output):
            with self.assertRaisesRegex(ValueError, "symbolic link"):
                package_cli.main()
        self.assertEqual(list(target.iterdir()), [])

    def test_symlinked_archive_input_is_refused(self):
        source = self.root / "docs" / "agent-cli.md"
        source.unlink()
        source.symlink_to("cli-reference.md")
        with patch.object(package_cli, "run", side_effect=self.tool_output):
            with self.assertRaisesRegex(ValueError, "not a regular file"):
                package_cli.main()

    def test_dirty_checkout_refuses_packaging_before_creating_output(self):
        with patch.object(package_cli, "run", return_value=" M docs/cli-reference.md"):
            with self.assertRaisesRegex(ValueError, "commit the candidate"):
                package_cli.main()
        self.assertFalse((self.root / "dist").exists())


if __name__ == "__main__":
    unittest.main()
