"""Bounded read-only cleanup observations; actual manager effects remain opt-in."""
import contextlib
import errno
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import verify


class SliceObservations(unittest.TestCase):
    def test_blocked_availability_waits_for_exact_recovered_receipt(self):
        case = verify.Qualification.__new__(verify.Qualification)
        intent = dict(incarnation='a'*32, sequence=7)
        pending = dict(**intent, state='settling', cleanup='pending')
        partial = dict(**intent, state='blocked', cleanup='pending')
        blocked = dict(**intent, state='blocked', cleanup='uncertain')
        case.request = Mock(side_effect=[{'result': {'availability': 'blocked', 'active': receipt}}
                                        for receipt in [None, pending, partial, blocked]])
        with patch.object(verify.time, 'sleep'):
            self.assertEqual(case.blocked_recovery(intent), blocked)
        self.assertEqual(case.request.call_args_list, [unittest.mock.call('reconcile')]*4)
        case.request = Mock(return_value={'result': {'availability': 'blocked',
                                                    'active': dict(blocked, sequence=8)}})
        with self.assertRaises(AssertionError):
            case.blocked_recovery(intent)

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


class CleanupContract(unittest.TestCase):
    def test_cleanup_stops_attempt_without_revert_and_proves_absence(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / 'root'
            output = base / 'output'
            (root / 'attempts' / 'launch' / 'work').mkdir(parents=True)
            (root / 'launcher.log').write_text('')
            output.mkdir()
            instance = uuid.uuid4().hex
            parent = 'slipstreamprocessing' + instance + '.slice'
            unit = parent.removesuffix('.slice') + '-' + uuid.uuid4().hex + '.slice'
            registry = dict(records={'1': dict(launch_id='launch', receipt=dict(runtime=dict(
                container_id=None, attempt_unit=unit)))})
            (root / 'registry.json').write_text(json.dumps(registry))

            case = verify.Qualification.__new__(verify.Qualification)
            case.root = root
            case.output = output
            case.parent = parent
            case.instance = instance
            case.web = 'owned-web-container'
            case.web_token = 'synthetic-token'
            case.stop = Mock()
            calls = []

            def fake_command(*arguments, **kwargs):
                calls.append(arguments)
                if arguments[:3] == ('systemctl', '--system', 'list-units'):
                    return ''
                if arguments[:3] == ('systemctl', '--system', 'show'):
                    return '/run/systemd/transient'
                return ''

            with patch.object(verify, 'command', side_effect=fake_command), \
                    patch.object(verify.os.path, 'ismount', return_value=False):
                case.cleanup()

            case.stop.assert_called_once()
            self.assertIn(('systemctl', 'stop', unit), calls)
            self.assertNotIn(('systemctl', 'revert', unit), calls)
            absence_call = ('systemctl', '--system', 'list-units', '--all', '--plain',
                            '--no-legend', '--no-pager', unit)
            self.assertIn(absence_call, calls)
            self.assertIn(('docker', 'rm', '--force', 'owned-web-container'), calls)
            self.assertIn(('systemctl', 'revert', parent), calls)
            self.assertIsNone(case.web_token)
            self.assertFalse(root.exists())
            self.assertTrue((output / 'final-registry.json').exists())

    def test_main_prints_pass_only_after_cleanup_succeeds(self):
        for fail_cleanup in [False, True]:
            with self.subTest(fail_cleanup=fail_cleanup):
                events = []
                cleanup_output = []
                stdout = io.StringIO()

                class FakeQualification:
                    def __init__(self, arguments):
                        self.results = [dict()]
                        self.output = Path(arguments.output)

                    def verify(self):
                        events.append('verify')

                    def cleanup(self):
                        events.append('cleanup')
                        cleanup_output.append(stdout.getvalue())
                        if fail_cleanup:
                            raise verify.subprocess.TimeoutExpired(['systemctl', 'revert'], 15)

                argv = ['verify.py', '--launcher', '/unused', '--worker-image',
                        'sha256:' + '0' * 64, '--web-image', 'sha256:' + '1' * 64,
                        '--output', '/evidence']
                with patch.object(verify, 'Qualification', FakeQualification), \
                        patch.object(verify.os, 'geteuid', return_value=0), \
                        patch.object(sys, 'argv', argv), \
                        contextlib.redirect_stdout(stdout):
                    if fail_cleanup:
                        with self.assertRaises(verify.subprocess.TimeoutExpired):
                            verify.main()
                    else:
                        verify.main()

                self.assertEqual(events, ['verify', 'cleanup'])
                self.assertEqual(cleanup_output, [''])
                if fail_cleanup:
                    self.assertEqual(stdout.getvalue(), '')
                else:
                    result = json.loads(stdout.getvalue())
                    self.assertEqual(result['status'], 'passed')
                    self.assertEqual(result['attempts'], 1)


if __name__ == '__main__':
    unittest.main()
