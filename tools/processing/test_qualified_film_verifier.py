import importlib.util
import copy
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
        receipt = dict(intent, plan=plan, limits={'memory_bytes': 8 * 1024**3},
                       evidence={'peak_bytes': 100}, qualification_failure=None)
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
        with patch.object(verifier.FILM.FilmQualification, 'terminal', return_value=changed):
            with self.assertRaises(AssertionError):
                probe.terminal(intent)
            changed['qualification_failure'] = 'peak-exceeded'
            self.assertEqual(probe.terminal(intent), changed)


if __name__ == '__main__':
    unittest.main()
