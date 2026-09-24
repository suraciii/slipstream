import copy
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location(
    'qualified_verifier', Path(__file__).with_name('verify-qualified-film.py'))
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)


class QualifiedVerifierTests(unittest.TestCase):
    def test_main_prints_pass_only_after_cleanup_succeeds(self):
        for fail_cleanup in [False, True]:
            with self.subTest(fail_cleanup=fail_cleanup), tempfile.TemporaryDirectory() as directory:
                launcher = Path(directory) / 'launcher'
                launcher.write_bytes(b'fixture launcher')
                fixture_id = 'f' * 32
                worker_image = 'sha256:' + 'a' * 64
                catalogue_data = b'catalogue'
                envelope_data = b'envelope'
                arguments = type('Arguments', (), dict(
                    launcher=str(launcher), worker_image=worker_image,
                    web_image='sha256:' + 'b' * 64, output=str(Path(directory) / 'output'),
                    catalogue='catalogue.json', envelope='envelope.json', memory_gib=8,
                    fixture=[fixture_id], expect_outcome='completed', expect_error=None,
                    expect_qualification_failure=None, alternate_envelope=None,
                    mismatched_envelope=None, recovery_fixture=None,
                    lifecycle_fixture=None, failure_fixture=None))()
                catalogue = dict(fixtures=[dict(id=fixture_id)])
                envelope = dict(catalogue_sha256=verifier.digest(catalogue_data),
                                image=worker_image,
                                launcher_sha256=verifier.digest(launcher.read_bytes()), cases=[])
                events = []
                cleanup_output = []
                stdout = io.StringIO()

                class FakeQualification:
                    def __init__(self, _arguments, *_documents):
                        self.results = [dict()]
                        self.refusals = []

                    def prepare(self):
                        events.append('prepare')

                    def verify(self):
                        events.append('verify')

                    def cleanup(self):
                        events.append('cleanup')
                        cleanup_output.append(stdout.getvalue())
                        if fail_cleanup:
                            raise verifier.subprocess.TimeoutExpired(['systemctl', 'revert'], 15)

                with patch.object(verifier.argparse.ArgumentParser, 'parse_args',
                                  return_value=arguments), \
                        patch.object(verifier.FILM, 'document',
                                     side_effect=[(catalogue_data, catalogue),
                                                  (envelope_data, envelope)]), \
                        patch.object(verifier, 'QualifiedFilmQualification', FakeQualification), \
                        patch.object(verifier.os, 'geteuid', return_value=0), \
                        contextlib.redirect_stdout(stdout):
                    if fail_cleanup:
                        with self.assertRaises(verifier.subprocess.TimeoutExpired):
                            verifier.main()
                    else:
                        verifier.main()

                self.assertEqual(events, ['prepare', 'verify', 'cleanup'])
                self.assertEqual(cleanup_output, [''])
                if fail_cleanup:
                    self.assertEqual(stdout.getvalue(), '')
                else:
                    result = json.loads(stdout.getvalue())
                    self.assertEqual(result, dict(status='passed', attempts=1, refusals=0))

    def test_independent_formula_consumes_authoritative_arithmetic_vectors(self):
        path = Path(__file__).resolve().parents[2] / 'design/schemas/processing-film-envelope-vectors.json'
        vectors = json.loads(path.read_text())['arithmetic']
        self.assertEqual(len(vectors), 7)
        for vector in vectors:
            with self.subTest(vector['name']):
                arguments = [vector[key] for key in ['known_required_bytes',
                             'empirical_ceiling_bytes', 'source_cache_bytes', 'safety_reserve_bytes']]
                if 'error' in vector:
                    with self.assertRaises(ValueError):
                        verifier.checked_requirement(*arguments)
                else:
                    actual = verifier.checked_requirement(*arguments)
                    self.assertEqual(actual, vector['required_bytes'])
                    self.assertEqual(actual <= vector['attempt_limit_bytes'], vector['fits'])

    def test_formula_rejects_coercions_nonpositive_terms_and_out_of_range_inputs(self):
        for index in range(4):
            for invalid in [True, 1.0, '1', -1, 2**64]:
                arguments = [1, 1, 1, 1]
                arguments[index] = invalid
                with self.subTest(index=index, value=invalid), self.assertRaises(ValueError):
                    verifier.checked_requirement(*arguments)
        for arguments in [(1, 0, 0, 1), (1, 1, 0, 0)]:
            with self.assertRaises(ValueError):
                verifier.checked_requirement(*arguments)

    def fake_refusal(self, output):
        probe = verifier.QualifiedFilmQualification.__new__(verifier.QualifiedFilmQualification)
        probe.output = output
        probe.refusals = []
        probe.intent = Mock(return_value=dict(sequence=7, workload={'fixture_id': 'a' * 32}))
        probe.request = Mock(side_effect=[{'error': {'code': 'resource-budget'}},
                                          {'result': {'next_sequence': 7}}])
        return probe

    def test_refusal_requires_no_registry_runtime_or_sequence_effects(self):
        before = dict(registry='old', attempts=[], cgroups=[], containers='', units='')
        with tempfile.TemporaryDirectory() as directory:
            probe = self.fake_refusal(Path(directory))
            probe.boundary_snapshot = Mock(side_effect=[before, dict(before)])
            probe.refused('a' * 32, 'resource-budget')
            evidence = json.loads((Path(directory) / 'refusals.json').read_text())
            self.assertEqual(evidence[0]['unchanged'], before)
            self.assertEqual(evidence[0]['intent']['sequence'], 7)
        for key in before:
            with self.subTest(changed=key), tempfile.TemporaryDirectory() as directory:
                probe = self.fake_refusal(Path(directory))
                after = dict(before, **{key: ['unexpected']})
                probe.boundary_snapshot = Mock(side_effect=[before, after])
                with self.assertRaisesRegex(AssertionError, 'ownership'):
                    probe.refused('a' * 32, 'resource-budget')
                self.assertFalse((Path(directory) / 'refusals.json').exists())
                evidence = json.loads((Path(directory) / 'refusal-boundary-1.json').read_text())
                self.assertEqual(evidence['before'], before)
                self.assertEqual(evidence['after'], after)
        with tempfile.TemporaryDirectory() as directory:
            probe = self.fake_refusal(Path(directory))
            probe.boundary_snapshot = Mock(side_effect=[before, dict(before)])
            probe.request.side_effect = [{'error': {'code': 'resource-budget'}},
                                         {'result': {'next_sequence': 8}}]
            with self.assertRaises(AssertionError):
                probe.refused('a' * 32, 'resource-budget')

    def test_mismatched_environment_waits_for_recovery_without_accepting_a_start(self):
        for original_policy in ['a' * 64, '0' * 64]:
            with self.subTest(policy=original_policy), tempfile.TemporaryDirectory() as directory:
                probe = self.fake_refusal(Path(directory))
                intent = dict(sequence=2, policy=original_policy, envelope='e' * 64,
                              workload={'fixture_id': 'a' * 32})
                probe.intent.return_value = intent
                responses = [{'error': {'code': 'unavailable'}},
                             {'error': {'code': 'unavailable'}},
                             {'error': {'code': 'incompatible-policy'}}]
                probe.request.side_effect = responses

                def delayed_recovery(ready):
                    self.assertFalse(ready())
                    self.assertFalse(ready())
                    self.assertTrue(ready())

                with patch.object(verifier, 'await_condition', side_effect=delayed_recovery):
                    probe.await_recovered_start_boundary('a' * 32)
                self.assertEqual(probe.request.call_count, 3)
                for call in probe.request.call_args_list:
                    self.assertEqual(call.args, ('start',))
                    self.assertEqual(call.kwargs['sequence'], 2)
                    self.assertNotEqual(call.kwargs['policy'], original_policy)
                    self.assertEqual(call.kwargs['envelope'], intent['envelope'])
                    self.assertEqual(call.kwargs['workload'], intent['workload'])
                evidence = json.loads((Path(directory) / 'recovery-probes-2.json').read_text())
                self.assertEqual(evidence['responses'], responses)

    def test_recovery_probe_keeps_unexpected_results_and_existing_deadline_failure(self):
        for response in [{'result': {'receipt': 'unexpected acceptance'}},
                         {'error': {'code': 'unknown-attempt'}},
                         {'error': {'code': 'unavailable'}}]:
            with self.subTest(response=response), tempfile.TemporaryDirectory() as directory:
                probe = self.fake_refusal(Path(directory))
                probe.intent.return_value = dict(sequence=2, policy='a' * 64)
                probe.request.return_value = response
                probe.request.side_effect = None

                def bounded_wait(ready):
                    self.assertFalse(ready())
                    raise AssertionError('condition did not become true within its fixed deadline')

                with patch.object(verifier, 'await_condition', side_effect=bounded_wait):
                    with self.assertRaises(AssertionError):
                        probe.await_recovered_start_boundary('a' * 32)
                evidence = json.loads((Path(directory) / 'recovery-probes-2.json').read_text())
                self.assertEqual(evidence['responses'], [response])

    def test_v3_requests_do_not_reuse_measurement_authority(self):
        probe = verifier.QualifiedFilmQualification.__new__(verifier.QualifiedFilmQualification)
        probe.instance = 'a' * 32
        probe.raw = Mock(return_value={})
        probe.request('start', envelope='b' * 64)
        payload = json.loads(probe.raw.call_args.args[0])
        self.assertEqual(payload, {'version': 3, 'instance': 'a' * 32,
                                  'op': 'start', 'envelope': 'b' * 64})
        self.assertNotIn('resource_model', payload)

    def test_terminal_binds_approval_and_detects_unreported_peak_miss(self):
        probe = verifier.QualifiedFilmQualification.__new__(verifier.QualifiedFilmQualification)
        probe.instance = 'a' * 32
        fixture_id = 'a' * 32
        case = dict(status='qualified', fixture_id=fixture_id,
                    empirical_ceiling_bytes=1024**3, safety_reserve_bytes=1024**3,
                    evidence_sha256='e' * 64)
        environment = {'cpu_sha256': 'c' * 64}
        probe.config = dict(envelope_sha256='b' * 64, catalogue_sha256='d' * 64)
        probe.envelope = dict(cases=[case], environment=environment)
        probe.fixtures = {fixture_id: dict(id=fixture_id, width=19, height=17,
                                          source=dict(kind='development-tiff', bytes=1234))}
        intent = dict(incarnation='f' * 32, sequence=1, workload={'fixture_id': fixture_id},
                      policy='1' * 64, bundle='2' * 64, catalogue='d' * 64, envelope='b' * 64)
        plan = dict(known_required_bytes=4 * 1024**3 + 65536,
                    source_cache_bytes=1234, storage_reserve_bytes=4 * 1024**3,
                    empirical_ceiling_bytes=case['empirical_ceiling_bytes'],
                    safety_reserve_bytes=case['safety_reserve_bytes'],
                    evidence_sha256=case['evidence_sha256'], width=19, height=17,
                    environment_sha256=verifier.digest(json.dumps(environment, sort_keys=True,
                                                       separators=(',', ':')).encode()),
                    attempt_limit_bytes=8 * 1024**3, envelope_sha256='b' * 64,
                    required_bytes=6 * 1024**3 + 1234)
        launch_id = 'b' * 32
        container_id = 'c' * 64
        attempt_unit = f'slipstreamprocessing{probe.instance}-{launch_id}.slice'
        events = {key: 0 for key in ('oom', 'oom_kill', 'oom_group_kill',
                                     'local_oom', 'local_oom_kill', 'local_oom_group_kill')}
        raw_events = 'low 0\nhigh 0\nmax 0\nfuture2 0\noom 0\noom_kill 0\noom_group_kill 0\n'
        snapshot = dict(
            cgroup_path=f'/sys/fs/cgroup/slipstreamprocessing{probe.instance}.slice/{attempt_unit}',
            cgroup_inode=10, unit_invocation='d' * 32, launch_id=launch_id,
            container_id=container_id, attempt_unit=attempt_unit,
            incarnation=intent['incarnation'], sequence=intent['sequence'],
            memory_peak_raw='100\n', memory_max_raw=f'{8 * 1024**3}\n',
            memory_swap_current_raw='0\n', memory_swap_max_raw='0\n',
            memory_events_raw=raw_events, memory_events_local_raw=raw_events)
        receipt = dict(intent, plan=plan, outcome='completed',
                       runtime=dict(launch_id=launch_id, container_id=container_id,
                                    attempt_unit=attempt_unit),
                       limits={'memory_bytes': 8 * 1024**3, 'swap_bytes': 0},
                       evidence={'peak_bytes': 100, 'populated': False,
                                 'attempt_after': events, 'terminal_snapshot': snapshot},
                       qualification_failure=None)
        with patch.object(verifier.FILM.FilmQualification, 'terminal', return_value=receipt):
            self.assertEqual(probe.terminal(intent), receipt)
        for key, value in [('empirical_ceiling_bytes', 2 * 1024**3),
                           ('safety_reserve_bytes', 2 * 1024**3), ('source_cache_bytes', 0),
                           ('evidence_sha256', '0' * 64), ('width', 20),
                           ('environment_sha256', '0' * 64)]:
            changed = copy.deepcopy(receipt)
            changed['plan'][key] = value
            changed['plan']['required_bytes'] = verifier.checked_requirement(
                *[changed['plan'][field] for field in ['known_required_bytes',
                  'empirical_ceiling_bytes', 'source_cache_bytes', 'safety_reserve_bytes']])
            with self.subTest(key=key), patch.object(verifier.FILM.FilmQualification, 'terminal',
                                                    return_value=changed), self.assertRaises(AssertionError):
                probe.terminal(intent)
        changed = copy.deepcopy(receipt)
        changed['evidence']['peak_bytes'] = case['empirical_ceiling_bytes'] + 1
        changed['evidence']['terminal_snapshot']['memory_peak_raw'] = (
            str(changed['evidence']['peak_bytes']) + '\n')
        with patch.object(verifier.FILM.FilmQualification, 'terminal', return_value=changed):
            with self.assertRaises(AssertionError):
                probe.terminal(intent)
            changed['qualification_failure'] = 'peak-exceeded'
            self.assertEqual(probe.terminal(intent), changed)

        tampered = (
            ('missing snapshot', lambda row: row['evidence'].update(terminal_snapshot=None)),
            ('identity', lambda row: row['evidence']['terminal_snapshot'].update(cgroup_inode=0)),
            ('peak', lambda row: row['evidence']['terminal_snapshot'].update(memory_peak_raw='101\n')),
            ('limit', lambda row: row['evidence']['terminal_snapshot'].update(memory_max_raw='1\n')),
            ('swap', lambda row: row['evidence']['terminal_snapshot'].update(memory_swap_max_raw='1\n')),
            ('overflow', lambda row: row['evidence']['terminal_snapshot'].update(
                memory_peak_raw=f'{2**64}\n')),
            ('events', lambda row: row['evidence']['terminal_snapshot'].update(
                memory_events_local_raw=raw_events.replace('oom_kill 0', 'oom_kill 1'))),
            ('aggregate bound', lambda row: row['evidence']['terminal_snapshot'].update(
                memory_events_raw='x' * 4096)),
        )
        for label, mutate in tampered:
            changed = copy.deepcopy(receipt)
            mutate(changed)
            with self.subTest(label=label), patch.object(
                    verifier.FILM.FilmQualification, 'terminal', return_value=changed), \
                    self.assertRaises(AssertionError):
                probe.terminal(intent)


if __name__ == '__main__':
    unittest.main()
