"""Bounded read-only cleanup observations; actual manager effects remain opt-in."""
from pathlib import Path
import errno
import tempfile
import unittest
from unittest.mock import patch

import verify


class SliceObservations(unittest.TestCase):
    def test_complete_absence_and_foreign_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            unit = 'independent-observation.slice'
            paths = '/run/systemd/transient ' + directory
            with patch.object(verify, 'command', side_effect=['', paths]):
                self.assertTrue(verify.attempt_absent('missing-parent.slice', unit,
                                                     verify.time.monotonic() + 5))
            Path(directory, unit + '.d').symlink_to('missing-foreign-target')
            with patch.object(verify, 'command', side_effect=['', paths]):
                self.assertFalse(verify.attempt_absent('missing-parent.slice', unit,
                                                      verify.time.monotonic() + 5))

    def test_command_errors_never_prove_absence(self):
        with patch.object(verify, 'command', side_effect=AssertionError('manager unavailable')):
            with self.assertRaisesRegex(AssertionError, 'manager unavailable'):
                verify.wait_attempt_absent('missing-parent.slice', 'attempt.slice')

    def test_unreadable_paths_never_prove_absence(self):
        for code in [errno.EACCES, errno.EIO, errno.ELOOP]:
            with self.subTest(errno=code), \
                    patch.object(verify, 'command', side_effect=['', '/run/systemd/transient']), \
                    patch.object(Path, 'lstat', side_effect=OSError(code, 'unreadable')):
                with self.assertRaises(OSError) as error:
                    verify.wait_attempt_absent('missing-parent.slice', 'attempt.slice')
                self.assertEqual(error.exception.errno, code)

    def test_queries_share_one_remaining_deadline(self):
        with patch.object(verify.time, 'monotonic', side_effect=[11, 12, 13]), \
                patch.object(verify, 'command', side_effect=['', '/run/systemd/transient']) as command:
            self.assertTrue(verify.attempt_absent('missing-parent.slice', 'attempt.slice', 15))
            self.assertEqual([call.kwargs['timeout'] for call in command.call_args_list], [4, 3])

    def test_expired_observation_never_starts_a_query(self):
        with patch.object(verify.time, 'monotonic', return_value=15), \
                patch.object(verify, 'command') as command:
            with self.assertRaisesRegex(AssertionError, 'expired'):
                verify.attempt_absent('missing-parent.slice', 'attempt.slice', 15)
            command.assert_not_called()


if __name__ == '__main__':
    unittest.main()
