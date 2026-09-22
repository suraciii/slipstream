"""Host-side safety checks; actual kernel/engine checks stay opt-in."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location('film_verifier', Path(__file__).with_name('verify-film.py'))
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)


class FixturePreparation(unittest.TestCase):
    def test_rejects_duplicate_noninteger_and_oversize_documents(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'document.json'
            for data in [b'{"a":1,"a":2}', b'{"a":1.0}', b'{"a":1e0}', b'{"a":NaN}', b'{"a":Infinity}']:
                path.write_bytes(data)
                with self.subTest(data=data), self.assertRaises(ValueError):
                    verifier.document(path, 64)
            path.write_bytes(b' ' * 65)
            with self.assertRaises(ValueError):
                verifier.document(path, 64)
            path.write_bytes(b'{"a":18446744073709551615}')
            raw, value = verifier.document(path, 64)
            self.assertEqual(json.loads(raw), value)

    def test_rejects_document_symlink_before_reading(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / 'source'
            target.write_text('{}')
            link = root / 'link'
            link.symlink_to(target)
            with self.assertRaises(OSError):
                verifier.document(link, 64)
            self.assertEqual(target.read_text(), '{}')

    def test_fixture_copy_is_exact_read_only_and_preserves_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, destination = root / 'source.tif', root / 'owned.tif'
            data = b'synthetic fixture bytes' * 4096
            source.write_bytes(data)
            expected = dict(bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
            before = source.stat()
            verifier.copy_fixture(source, destination, expected)
            self.assertEqual(destination.read_bytes(), data)
            self.assertEqual(destination.stat().st_mode & 0o777, 0o444)
            self.assertEqual(source.read_bytes(), data)
            self.assertEqual(source.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(source.stat().st_ino, before.st_ino)

    def test_refuses_aliases_wrong_digest_and_existing_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source.tif'
            source.write_bytes(b'original fixture')
            expected = dict(bytes=source.stat().st_size,
                            sha256=hashlib.sha256(source.read_bytes()).hexdigest())
            link = root / 'symlink'
            link.symlink_to(source)
            with self.assertRaises(OSError):
                verifier.copy_fixture(link, root / 'uncreated', expected)
            alias = root / 'hardlink'
            os.link(source, alias)
            with self.assertRaises(ValueError):
                verifier.copy_fixture(source, root / 'uncreated', expected)
            alias.unlink()
            with self.assertRaises(ValueError):
                verifier.copy_fixture(source, root / 'wrong', dict(expected, sha256='0'*64))
            destination = root / 'existing'
            destination.write_bytes(b'protected')
            with self.assertRaises(FileExistsError):
                verifier.copy_fixture(source, destination, expected)
            self.assertEqual(destination.read_bytes(), b'protected')
            self.assertEqual(source.read_bytes(), b'original fixture')
            self.assertFalse((root / 'uncreated').exists())

if __name__ == '__main__':
    unittest.main()
