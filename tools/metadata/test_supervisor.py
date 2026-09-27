import importlib.util
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("supervisor.py")
_spec = importlib.util.spec_from_file_location("slipstream_metadata_supervisor", MODULE_PATH)
assert _spec and _spec.loader
_supervisor_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_supervisor_module)


class SupervisorCleanupTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix="slipstream-supervisor-test-"))
        self.root = self.base / "library"
        self.runtime = self.base / "runtime"
        self.root.mkdir()
        self.runtime.mkdir()
        supervisor = object.__new__(_supervisor_module.Supervisor)
        supervisor.root = self.root
        supervisor.runtime = self.runtime
        supervisor.marker = self.runtime / "fenced"
        supervisor.unit = "slipstream-metadata-test.service"
        supervisor.publication_record = self.runtime / "publication.json"
        supervisor.publication_pending = self.runtime / "publication.json.new"
        supervisor.config = {"writerUid": os.getuid(), "writerGid": os.getgid()}
        self.supervisor = supervisor

    def tearDown(self):
        shutil.rmtree(self.base, ignore_errors=True)

    def test_no_record_startup_preserves_matching_files_including_same_writer(self):
        matching = [
            self.root / ".slipstream-sidecar-foreign.tmp",
            self.root / ".slipstream-sidecar-same-writer.tmp",
        ]
        snapshots = {}
        for path in matching:
            path.write_bytes(path.name.encode())
            snapshots[path] = self._snapshot(path)

        self.supervisor.recover()

        for path, snapshot in snapshots.items():
            self._assert_snapshot(path, snapshot)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_fenced_record_removes_only_exact_artifact_and_preserves_sentinels(self):
        supervisor, base = self._production_supervisor()
        try:
            token = "a" * 64
            self._write_ledger(supervisor, token)
            expected = supervisor.root / supervisor._temporary_name(token)
            self._owned_file(expected, 1, 0, 0o600, b"owned staged publication")
            sentinels = {
                supervisor.root / ".slipstream-sidecar-root-owned.tmp": (0, 0, 0o440),
                supervisor.root / ".slipstream-sidecar-same-writer.tmp": (1, 0, 0o600),
                supervisor.root / ".slipstream-sidecar-foreign-uid.tmp": (2, 0, 0o600),
                supervisor.root / supervisor._temporary_name("b" * 64): (1, 0, 0o600),
            }
            snapshots = {}
            for path, identity in sentinels.items():
                self._owned_file(path, *identity, content=b"must survive")
                snapshots[path] = self._snapshot(path)

            with self._fence(supervisor):
                supervisor.discard_staged_publication()

            self.assertFalse(expected.exists())
            for path, snapshot in snapshots.items():
                self._assert_snapshot(path, snapshot)
            self.assertFalse(supervisor.publication_record.exists())
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_no_marker_ledger_refuses_and_preserves_artifact(self):
        supervisor, base = self._production_supervisor()
        try:
            token = "c" * 64
            self._write_ledger(supervisor, token)
            expected = supervisor.root / supervisor._temporary_name(token)
            self._owned_file(expected, 1, 0, 0o600, b"abandoned")
            before = self._snapshot(expected)
            supervisor.marker.unlink()

            with self.assertRaisesRegex(_supervisor_module.Refusal, "without an active fence"):
                supervisor.recover()

            self._assert_snapshot(expected, before)
            self.assertTrue(supervisor.publication_record.exists())
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_active_smb_refuses_cleanup_at_properties_boundary(self):
        supervisor, base = self._production_supervisor()
        try:
            token = "d" * 64
            self._write_ledger(supervisor, token)
            expected = supervisor.root / supervisor._temporary_name(token)
            self._owned_file(expected, 1, 0, 0o600, b"abandoned")
            before = self._snapshot(expected)
            supervisor.marker.touch(mode=0o600)

            with patch.object(
                _supervisor_module,
                "properties",
                return_value={"LoadState": "loaded", "ActiveState": "active", "ControlGroup": ""},
            ), patch.object(supervisor, "identity_isolation"):
                with self.assertRaisesRegex(_supervisor_module.Refusal, "masked and stopped"):
                    supervisor.discard_staged_publication()

            self._assert_snapshot(expected, before)
            self.assertTrue(supervisor.publication_record.exists())
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_parent_inode_or_symlink_swap_refuses_and_preserves_artifact(self):
        supervisor, base = self._production_supervisor()
        old_root = base / "library-old"
        replacement = supervisor.root
        try:
            token = "e" * 64
            self._write_ledger(supervisor, token)
            expected = supervisor.root / supervisor._temporary_name(token)
            self._owned_file(expected, 1, 0, 0o600, b"abandoned")
            before = self._snapshot(expected)
            os.rename(replacement, old_root)
            replacement.mkdir()
            os.chown(replacement, 0, 0)
            os.chmod(replacement, 0o3770)

            with self._fence(supervisor):
                with self.assertRaisesRegex(_supervisor_module.Refusal, "parent identity"):
                    supervisor.discard_staged_publication()
            self.assertTrue(supervisor.publication_record.exists())
            self._assert_snapshot(old_root / expected.name, before)

            replacement.rmdir()
            os.symlink(old_root, replacement)
            with self._fence(supervisor):
                with self.assertRaisesRegex(_supervisor_module.Refusal, "cannot be safely reopened"):
                    supervisor.discard_staged_publication()
            self.assertTrue(supervisor.publication_record.exists())
            self._assert_snapshot(old_root / expected.name, before)
        finally:
            if replacement.is_symlink():
                replacement.unlink()
            elif replacement.exists():
                shutil.rmtree(replacement)
            if old_root.exists():
                os.rename(old_root, replacement)
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_corrupt_record_refuses_without_touching_matching_file(self):
        supervisor, base = self._production_supervisor()
        try:
            supervisor.marker.touch(mode=0o600)
            supervisor.publication_record.write_bytes(b"not-json")
            os.chmod(supervisor.publication_record, 0o600)
            sentinel = supervisor.root / ".slipstream-sidecar-corrupt.tmp"
            self._owned_file(sentinel, 1, 0, 0o600, b"must survive")
            before = self._snapshot(sentinel)

            with self._fence(supervisor):
                with self.assertRaisesRegex(_supervisor_module.Refusal, "publication record is invalid"):
                    supervisor.discard_staged_publication()

            self._assert_snapshot(sentinel, before)
            self.assertTrue(supervisor.publication_record.exists())
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_recorded_artifact_wrong_owner_refuses_and_preserves_ledger(self):
        supervisor, base = self._production_supervisor()
        try:
            token = "a" * 64
            self._write_ledger(supervisor, token)
            expected = supervisor.root / supervisor._temporary_name(token)
            self._owned_file(expected, 2, 0, 0o600, b"foreign artifact")
            before = self._snapshot(expected)

            with self._fence(supervisor):
                with self.assertRaisesRegex(_supervisor_module.Refusal, "identity is invalid"):
                    supervisor.discard_staged_publication()

            self._assert_snapshot(expected, before)
            self.assertTrue(supervisor.publication_record.exists())
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_pending_record_recovery_removes_only_runtime_pending_state(self):
        supervisor, base = self._production_supervisor()
        try:
            supervisor.marker.touch(mode=0o600)
            supervisor.publication_pending.write_bytes(b"pending-before-helper")
            os.chmod(supervisor.publication_pending, 0o600)
            sentinel = supervisor.root / ".slipstream-sidecar-pending.tmp"
            self._owned_file(sentinel, 1, 0, 0o600, b"must survive")
            before = self._snapshot(sentinel)
            supervisor.kill_descendants = lambda: None
            supervisor.release = lambda: None

            supervisor.recover()

            self.assertFalse(supervisor.publication_pending.exists())
            self.assertFalse(supervisor.publication_record.exists())
            self._assert_snapshot(sentinel, before)
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @unittest.skipUnless(os.geteuid() == 0, "requires production root-owned directory admission")
    def test_missing_artifact_after_rename_clears_ledger_under_fence(self):
        supervisor, base = self._production_supervisor()
        try:
            token = "f" * 64
            self._write_ledger(supervisor, token)
            supervisor.marker.touch(mode=0o600)
            sentinel = supervisor.root / ".slipstream-sidecar-after-rename.tmp"
            self._owned_file(sentinel, 1, 0, 0o600, b"must survive")
            before = self._snapshot(sentinel)

            with self._fence(supervisor):
                supervisor.discard_staged_publication()

            self.assertFalse(supervisor.publication_record.exists())
            self._assert_snapshot(sentinel, before)
        finally:
            shutil.rmtree(base, ignore_errors=True)

    @staticmethod
    def _snapshot(path):
        facts = path.stat()
        return path.read_bytes(), facts.st_dev, facts.st_ino, facts.st_uid, facts.st_gid, facts.st_mode & 0o7777

    def _assert_snapshot(self, path, snapshot):
        self.assertTrue(path.exists(), path)
        content, device, inode, uid, gid, mode = snapshot
        facts = path.stat()
        self.assertEqual(path.read_bytes(), content)
        self.assertEqual((facts.st_dev, facts.st_ino, facts.st_uid, facts.st_gid, facts.st_mode & 0o7777), (device, inode, uid, gid, mode))

    @staticmethod
    def _owned_file(path, uid, gid, mode, content):
        path.write_bytes(content)
        os.chown(path, uid, gid)
        os.chmod(path, mode)

    @contextmanager
    def _fence(self, supervisor):
        with patch.object(
            _supervisor_module,
            "properties",
            return_value={"LoadState": "masked", "ActiveState": "inactive", "ControlGroup": ""},
        ), patch.object(supervisor, "identity_isolation"):
            yield

    def _write_ledger(self, supervisor, token):
        supervisor.marker.touch(mode=0o600)
        with self._fence(supervisor):
            supervisor._write_publication_record("photo.ARW", token)

    def _production_supervisor(self):
        base = Path(tempfile.mkdtemp(prefix="slipstream-supervisor-test-", dir="/run"))
        root = base / "library"
        runtime = base / "runtime"
        root.mkdir()
        runtime.mkdir()
        os.chown(base, 0, 0)
        os.chmod(base, 0o755)
        os.chown(runtime, 0, 0)
        os.chmod(runtime, 0o700)
        os.chown(root, 0, 0)
        os.chmod(root, 0o3770)
        supervisor = object.__new__(_supervisor_module.Supervisor)
        supervisor.root = root
        supervisor.runtime = runtime
        supervisor.marker = runtime / "fenced"
        supervisor.unit = "slipstream-metadata-test.service"
        supervisor.publication_record = runtime / "publication.json"
        supervisor.publication_pending = runtime / "publication.json.new"
        supervisor.config = {"writerUid": 1, "writerGid": 0}
        return supervisor, base


if __name__ == "__main__":
    unittest.main()
