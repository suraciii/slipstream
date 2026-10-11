"""Pure bundle pinning and exclusive backup/isolated restore coverage."""
import hashlib
import importlib.util
import json
import tempfile
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).parent

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

backup = load("tool_backup", HERE / "backup.py")
restore = load("tool_restore", HERE / "restore.py")
manifest = load("edit_manifest", HERE / "photo/edit_manifest.py")


class PackagingTests(unittest.TestCase):
    def test_exact_manifest_covers_assets_and_embedded_profiles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "native/bin").mkdir(parents=True)
            (root / "native/share/darktable").mkdir(parents=True)
            (root / "native/bin/darktable-mcp").write_bytes(b"native")
            metadata = {"protocol": "2025-06-18", "tools": [], "discovery": {"schemaVersion": "darktable-edit-1", "outputProfiles": {"srgb": "a" * 64}}}
            report = manifest.create_manifest(root, metadata, "b" * 40)
            encoded = (root / "edit-manifest.json").read_bytes()
            self.assertEqual(report["bundle"], hashlib.sha256(encoded).hexdigest())
            document = json.loads(encoded)
            self.assertEqual(document["outputProfiles"], metadata["discovery"]["outputProfiles"])
            self.assertEqual(document["files"]["native/bin/darktable-mcp"], hashlib.sha256(b"native").hexdigest())
            (root / "linked").symlink_to("native/bin/darktable-mcp")
            with self.assertRaisesRegex(ValueError, "symlinks"):
                manifest.create_manifest(root, metadata, "b" * 40)

    def test_backup_restore_preserves_complete_bytes_before_rebase(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = root / "state"
            (state / "workspaces/ws/input").mkdir(parents=True)
            (state / "workspaces/ws/input/raw").write_bytes(b"read-only input")
            (state / "state.sqlite-wal").write_bytes(b"wal")
            (state / "empty.lock").write_bytes(b"")
            archive = root / "backup.tar"
            backup.backup([state], archive)
            with patch.object(restore.subprocess, "run") as rebase:
                result = restore.restore(archive, root / "restored", state, "server")
                restored = Path(result["stateDirectory"])
                self.assertEqual((restored / "workspaces/ws/input/raw").read_bytes(), b"read-only input")
                self.assertEqual((restored / "state.sqlite-wal").read_bytes(), b"wal")
                self.assertEqual((restored / "empty.lock").read_bytes(), b"")
                restored_manifest = json.loads(Path(result["manifest"]).read_text())
                empty = next(item for item in restored_manifest["files"] if item["relativePath"] == "root-0/empty.lock")
                self.assertEqual(empty, {"relativePath": "root-0/empty.lock", "sha256": hashlib.sha256(b"").hexdigest(), "byteLength": 0})
                self.assertEqual(rebase.call_args.args[0], ["server", "__restore-rebase", "--state", str(restored), "--manifest", result["manifest"]])
                self.assertEqual(rebase.call_args.args[0][:2], ["server", "__restore-rebase"])
            with self.assertRaises(FileExistsError):
                restore.restore(archive, root / "restored", state, "server")

    def test_backup_refuses_writer_lease_and_never_publishes_archive(self):
        import fcntl
        import os
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            state = root / "state"
            state.mkdir()
            descriptor = backup.open_directory(state)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_SH)
                with self.assertRaises(BlockingIOError):
                    backup.backup([state], root / "backup.tar")
                self.assertFalse((root / "backup.tar").exists())
            finally:
                os.close(descriptor)


if __name__ == "__main__":
    unittest.main()
