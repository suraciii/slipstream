#!/usr/bin/env python3
"""Verify explicit root-approved fixture envelopes through the v3 executor.

This opt-in kernel verifier never fits or silently modifies an envelope. Supply
reviewed documents and independently referenced fixtures. Deliberately failing
qualification documents are test inputs, not supported production budgets.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess

from verify import Qualification, await_condition, command


FILM_PATH = Path(__file__).with_name('verify-film.py')
SPEC = importlib.util.spec_from_file_location('film_measurement_verifier', FILM_PATH)
FILM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FILM)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def checked_requirement(known, empirical, source, reserve):
    """Independent exact-integer evaluation of the accepted total formula."""
    values = [known, empirical, source, reserve]
    if any(type(value) is not int or not 0 <= value < 2**64 for value in values):
        raise ValueError('invalid unsigned byte count')
    if empirical == 0 or reserve == 0:
        raise ValueError('positive empirical ceiling and reserve required')
    raw = empirical + 4 * FILM.GIB + source
    result = max(known, raw) + reserve
    if raw >= 2**64 or result >= 2**64:
        raise ValueError('total requirement overflow')
    return result


def verify_terminal_snapshot(receipt, instance):
    evidence = receipt['evidence']
    runtime = receipt['runtime']
    snapshot = evidence['terminal_snapshot']
    if runtime is None or runtime['container_id'] is None:
        assert snapshot is None, receipt
        return
    assert isinstance(snapshot, dict), receipt
    raw_names = ('memory_peak_raw', 'memory_max_raw', 'memory_swap_current_raw',
                 'memory_swap_max_raw', 'memory_events_raw', 'memory_events_local_raw')
    assert set(snapshot) == {
        'cgroup_path', 'cgroup_inode', 'unit_invocation', 'launch_id',
        'container_id', 'attempt_unit', 'incarnation', 'sequence', *raw_names}, receipt
    assert all(type(snapshot.get(name)) is str and snapshot[name].isascii()
               for name in raw_names), receipt
    assert sum(len(snapshot[name].encode('ascii')) for name in raw_names) <= 4096, receipt

    unit = runtime['attempt_unit']
    assert snapshot['cgroup_path'] == (
        f'/sys/fs/cgroup/slipstreamprocessing{instance}.slice/{unit}'), receipt
    assert unit == f'slipstreamprocessing{instance}-{runtime["launch_id"]}.slice', receipt
    assert (type(snapshot['cgroup_inode']) is int and snapshot['cgroup_inode'] > 0
            and len(snapshot['unit_invocation']) == 32
            and all(char in '0123456789abcdef' for char in snapshot['unit_invocation'])), receipt
    for key in ('launch_id', 'container_id', 'attempt_unit'):
        assert snapshot[key] == runtime[key], receipt
    for key in ('incarnation', 'sequence'):
        assert snapshot[key] == receipt[key], receipt

    def number(value):
        assert re.fullmatch(r'[0-9]+\n', value), receipt
        parsed = int(value[:-1])
        assert parsed < 2**64, receipt
        return parsed

    def events(value):
        parsed = {}
        for line in value.splitlines(keepends=True):
            match = re.fullmatch(r'([a-z0-9_]+) ([0-9]+)\n', line)
            assert match is not None and match[1] not in parsed, receipt
            parsed[match[1]] = int(match[2])
            assert parsed[match[1]] < 2**64, receipt
        assert all(key in parsed for key in ('oom', 'oom_kill', 'oom_group_kill')), receipt
        return parsed

    peak = number(snapshot['memory_peak_raw'])
    maximum = number(snapshot['memory_max_raw'])
    swap_current = number(snapshot['memory_swap_current_raw'])
    swap_max = number(snapshot['memory_swap_max_raw'])
    all_events = events(snapshot['memory_events_raw'])
    local_events = events(snapshot['memory_events_local_raw'])
    assert peak == evidence['peak_bytes'] and evidence['populated'] is False, receipt
    assert maximum == receipt['limits']['memory_bytes'] == receipt['plan']['attempt_limit_bytes'], receipt
    assert swap_current == swap_max == receipt['limits']['swap_bytes'] == 0, receipt
    after = evidence['attempt_after']
    assert isinstance(after, dict), receipt
    for key in ('oom', 'oom_kill', 'oom_group_kill'):
        assert all_events[key] == after[key] and local_events[key] == after['local_' + key], receipt
    if receipt['outcome'] == 'completed':
        assert all(after[key] == 0 for key in
                   ('oom', 'oom_kill', 'oom_group_kill', 'local_oom',
                    'local_oom_kill', 'local_oom_group_kill')), receipt


class QualifiedFilmQualification(FILM.FilmQualification):
    def __init__(self, arguments, catalogue_data, catalogue, envelope_data, envelope):
        Qualification.__init__(self, arguments)
        self.sources = {str(path): digest(path.read_bytes()) for path in
                        [Path(__file__), FILM_PATH, Path(__file__).with_name('verify.py')]}
        (self.root / 'launcher.log').touch(mode=0o600)
        self.catalogue = catalogue
        self.fixtures = {item['id']: item for item in catalogue['fixtures']}
        self.envelope = envelope
        self.refusals = []
        self.injected_drift = set()
        self.config.update(version=3, mode='film-qualified-fixtures', peer_uid=0,
                           memory_bytes=arguments.memory_gib * FILM.GIB,
                           catalogue_sha256=digest(catalogue_data),
                           envelope_sha256=digest(envelope_data))
        for name, data in [('catalogue.json', catalogue_data), ('envelope.json', envelope_data)]:
            path = self.root / name
            path.write_bytes(data)
            path.chmod(0o400)
        self.save_config()

    def request(self, operation, **fields):
        return self.raw(json.dumps(dict(version=3, instance=self.instance, op=operation,
                                       **fields), separators=(',', ':')).encode())

    def start(self, availability='available'):
        assert self.process is None
        self.log = (self.root / 'launcher.log').open('ab')
        self.process = subprocess.Popen([str(self.launcher), '--config',
                                         str(self.root / 'config.json')],
                                        stdout=self.log, stderr=subprocess.STDOUT)

        def ready():
            assert self.process.poll() is None, 'launcher exited'
            try:
                result = self.request('reconcile').get('result')
                if result and result['availability'] == availability:
                    assert result['capability'] == 'film-qualified-fixtures-only', result
                    return result
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            return None

        self.capability = await_condition(ready)
        return self.capability

    def intent(self, fixture_id):
        capability = self.request('reconcile')['result']
        assert capability['capability'] == 'film-qualified-fixtures-only', capability
        return dict(incarnation=capability['incarnation'], sequence=capability['next_sequence'],
                    policy=capability['policy'], bundle=capability['bundle'],
                    catalogue=capability['catalogue'], envelope=capability['envelope'],
                    workload=dict(kind='film-fixture', fixture_id=fixture_id))

    def boundary_snapshot(self):
        """Observe the complete no-effect contract around a refused Start."""
        registry = self.root / 'registry.json'
        registry_data = registry.read_bytes()
        attempts = self.root / 'attempts'
        parent = Path('/sys/fs/cgroup', self.parent)
        return dict(registry=digest(registry_data), registry_state=json.loads(registry_data),
                    attempts=sorted(str(path.relative_to(attempts))
                                    for path in attempts.rglob('*')) if attempts.exists() else [],
                    cgroups=sorted(str(path.relative_to(parent))
                                   for path in parent.rglob('*') if path.is_dir()),
                    containers=command('docker', 'ps', '--all', '--quiet', '--no-trunc',
                                       '--filter', 'label=slipstream.processing.instance=' + self.instance),
                    units=command('systemctl', '--system', 'list-units', '--all', '--plain',
                                  '--no-legend', '--no-pager',
                                  'slipstreamprocessing' + self.instance + '-*.slice'))

    def refused(self, fixture_id, expected, **changed):
        intent = dict(self.intent(fixture_id), **changed)
        before = self.boundary_snapshot()
        response = self.request('start', **intent)
        after = self.boundary_snapshot()
        # Keep the differing ownership evidence even when the assertion stops
        # this verifier and its finally block removes the private runtime.
        path = self.output / ('refusal-boundary-' + str(len(self.refusals) + 1) + '.json')
        with path.open('x') as evidence:
            json.dump(dict(intent=intent, response=response, before=before, after=after),
                      evidence, indent=2)
        assert response['error']['code'] == expected, response
        assert after == before, 'refused Start changed execution ownership'
        capability = self.request('reconcile')['result']
        assert capability['next_sequence'] == intent['sequence'], capability
        self.refusals.append(dict(intent=intent, response=response, unchanged=before))
        (self.output / 'refusals.json').write_text(json.dumps(self.refusals, indent=2))
        return response

    def await_recovered_start_boundary(self, fixture_id):
        """Distinguish completed recovery from its initial blocked capability.

        A wrong policy is rejected after ownership recovery permits new Start
        validation, but before the current environment is checked. It cannot
        create an attempt. A mismatched environment remains blocked on both
        sides of recovery, so that capability alone is not a recovery barrier.
        """
        intent = dict(self.intent(fixture_id))
        expected_policy = intent['policy']
        intent['policy'] = '0' * 64 if expected_policy != '0' * 64 else '1' * 64
        observations = []

        def recovered():
            response = self.request('start', **intent)
            observations.append(response)
            code = response.get('error', {}).get('code')
            assert code in ('unavailable', 'incompatible-policy'), response
            return code == 'incompatible-policy'

        try:
            await_condition(recovered)
        finally:
            path = self.output / ('recovery-probes-' + str(intent['sequence']) + '.json')
            with path.open('x') as evidence:
                json.dump(dict(expected_policy=expected_policy, intent=intent,
                               responses=observations), evidence, indent=2)

    def terminal(self, intent):
        receipt = super().terminal(intent)
        for key in ['incarnation', 'sequence', 'workload', 'policy', 'bundle', 'catalogue', 'envelope']:
            assert receipt[key] == intent[key], ('changed captured authority', key, receipt)
        assert receipt['envelope'] == self.config['envelope_sha256']
        assert receipt['catalogue'] == self.config['catalogue_sha256']
        fixture = self.fixtures[intent['workload']['fixture_id']]
        case = next(case for case in self.envelope['cases'] if case['fixture_id'] == fixture['id'])
        assert case['status'] == 'qualified'
        plan = receipt['plan']
        for key in ['empirical_ceiling_bytes', 'safety_reserve_bytes', 'evidence_sha256']:
            assert plan[key] == case[key], ('changed approved envelope case', key, receipt)
        for key in ['width', 'height']:
            assert plan[key] == fixture[key], ('changed fixture geometry', key, receipt)
        assert plan['source_cache_bytes'] == fixture['source'].get('bytes', 0), receipt
        environment_hash = digest(json.dumps(self.envelope['environment'], sort_keys=True,
                                             separators=(',', ':')).encode())
        assert plan['environment_sha256'] == environment_hash, receipt
        required = checked_requirement(plan['known_required_bytes'],
                                       plan['empirical_ceiling_bytes'],
                                       plan['source_cache_bytes'], plan['safety_reserve_bytes'])
        assert required == plan['required_bytes'] <= receipt['limits']['memory_bytes'], receipt
        assert plan['attempt_limit_bytes'] == receipt['limits']['memory_bytes'], receipt
        assert plan['envelope_sha256'] == receipt['envelope'], receipt
        assert plan['storage_reserve_bytes'] == 4 * FILM.GIB, receipt
        verify_terminal_snapshot(receipt, self.instance)
        if receipt['qualification_failure'] == 'peak-exceeded':
            assert receipt['evidence']['peak_bytes'] > plan['empirical_ceiling_bytes'], receipt
        if (intent['sequence'] not in getattr(self, 'injected_drift', set())
                and receipt['evidence']['peak_bytes'] is not None
                and receipt['evidence']['peak_bytes'] > case['empirical_ceiling_bytes']):
            assert receipt['qualification_failure'] == 'peak-exceeded', receipt
        return receipt

    def permit_drift(self, fixture_id, phase):
        """Change only this owned attempt's quota while its heavy permit is held."""
        intent = self.intent(fixture_id)
        self.arm(intent, phase)
        accepted = self.request('start', **intent)['result']['receipt']
        marker = self.film_marker(intent)
        assert marker['phase'] == phase and marker['launch_id'] == accepted['runtime']['launch_id']
        current = self.receipt(intent)
        runtime = current['runtime']
        for key in ['launch_id', 'attempt_unit']:
            assert runtime[key] == accepted['runtime'][key]
        assert runtime['container_id'] is not None and len(runtime['container_id']) == 64
        assert all(character in '0123456789abcdef' for character in runtime['container_id'])
        group = Path('/sys/fs/cgroup', self.parent, runtime['attempt_unit'],
                     'docker-' + runtime['container_id'] + '.scope', 'workload')
        info = json.loads(command('docker', 'inspect', runtime['container_id']))[0]
        assert info['Id'] == runtime['container_id']
        assert info['Config']['Labels']['slipstream.processing.launch'] == runtime['launch_id']
        assert info['Config']['Labels']['slipstream.processing.instance'] == self.instance
        before = (group / 'cpu.max').read_text().strip()
        assert before == '400000 100000'
        # This leaf belongs to the launcher. Do not alter manager-owned ancestors
        # or create systemd drop-ins merely to inject the readback mismatch.
        (group / 'cpu.max').write_text('300000 100000')
        after = (group / 'cpu.max').read_text().strip()
        assert after == '300000 100000'
        self.injected_drift.add(intent['sequence'])
        self.release(marker)
        receipt = self.terminal(intent)
        assert receipt['outcome'] == 'interrupted' and receipt['detail'] is None, receipt
        assert receipt['qualification_failure'] is None, receipt
        assert receipt['phase'] != 'engine' and receipt['result'] is None, receipt
        record = json.loads((self.root / 'registry.json').read_text())['records'][str(intent['sequence'])]
        assert record['film']['qualification_observation_valid'] is False, record
        (self.output / (phase + '-quota-drift.json')).write_text(json.dumps(
            dict(marker=marker, before_cpu_max=before, injected_cpu_max=after,
                 measurement_eligible=False, receipt=receipt), indent=2))
        self.disarm()
        recovered = self.run(fixture_id, 'completed')
        assert recovered['qualification_failure'] is None, recovered

    def switch_envelope(self, data, envelope, availability='available'):
        self.stop()
        path = self.root / 'envelope.json'
        path.write_bytes(data)
        path.chmod(0o400)
        self.config['envelope_sha256'] = digest(data)
        self.envelope = envelope
        self.save_config()
        return self.start(availability)

    def verify(self):
        initial_availability = ('available' if any(case['status'] == 'qualified'
                                                  for case in self.envelope['cases'])
                                else 'unqualified')
        self.start(initial_availability)
        self.start_web()
        # A v3 instance cannot grant v1 native or v2 measurement authority.
        for version in (1, 2):
            response = self.raw(json.dumps(dict(version=version, instance=self.instance,
                                                op='reconcile')).encode())
            assert response['error']['code'] == 'invalid-request', response
        for fixture_id in self.arguments.fixture:
            if self.arguments.expect_error:
                self.refused(fixture_id, self.arguments.expect_error)
                continue
            intent = self.intent(fixture_id)
            accepted = self.request('start', **intent)['result']['receipt']
            receipt = self.terminal(intent)
            assert receipt['outcome'] == self.arguments.expect_outcome, receipt
            assert receipt['qualification_failure'] == self.arguments.expect_qualification_failure, receipt
            assert accepted['plan'] == receipt['plan'], 'captured plan changed during execution'
            assert self.request('start', **intent)['result']['receipt'] == receipt
            if receipt['qualification_failure'] is not None:
                self.refused(fixture_id, 'unqualified-envelope')
            if self.arguments.recovery_fixture:
                recovered = self.run(self.arguments.recovery_fixture, 'completed')
                assert recovered['qualification_failure'] is None, recovered
            # The same withdrawn case stays fenced after an ordinary restart.
            availability = self.request('reconcile')['result']['availability']
            self.stop()
            self.start(availability)
            assert self.request('start', **intent)['result']['receipt'] == receipt
            if receipt['qualification_failure'] is not None:
                self.refused(fixture_id, 'unqualified-envelope')
            if self.arguments.alternate_envelope:
                assert receipt['qualification_failure'] is not None
                original_data = (self.root / 'envelope.json').read_bytes()
                original_envelope = self.envelope
                data, envelope = FILM.document(self.arguments.alternate_envelope, 131072)
                assert digest(data) != digest(original_data)
                self.switch_envelope(data, envelope)
                assert self.request('start', **intent)['result']['receipt'] == receipt
                alternate = self.run(fixture_id, 'completed')
                assert alternate['qualification_failure'] is None, alternate
                self.switch_envelope(original_data, original_envelope, availability)
                self.refused(fixture_id, 'unqualified-envelope')
                assert self.request('start', **intent)['result']['receipt'] == receipt
            if self.arguments.mismatched_envelope:
                original_data = (self.root / 'envelope.json').read_bytes()
                original_envelope = self.envelope
                current_availability = self.request('reconcile')['result']['availability']
                data, envelope = FILM.document(self.arguments.mismatched_envelope, 131072)
                assert envelope['environment'] != original_envelope['environment']
                self.switch_envelope(data, envelope, 'blocked')
                assert self.request('start', **intent)['result']['receipt'] == receipt
                self.await_recovered_start_boundary(fixture_id)
                self.refused(fixture_id, 'unavailable')
                self.switch_envelope(original_data, original_envelope, current_availability)
                assert self.request('start', **intent)['result']['receipt'] == receipt
        if self.arguments.lifecycle_fixture:
            for phase in ['after-stage-release-intent', 'after-engine-release-intent']:
                self.permit_drift(self.arguments.lifecycle_fixture, phase)
            for phase in ['after-stage-release-intent', 'after-snapshot-sealed',
                          'after-engine-release-intent', 'after-validated-result']:
                self.crash_film(self.arguments.lifecycle_fixture, phase)
            self.cancel_sealed(self.arguments.lifecycle_fixture)
            assert all(item['qualification_failure'] is None for item in self.results
                       if item['workload']['fixture_id'] == self.arguments.lifecycle_fixture)
        if self.arguments.failure_fixture:
            for inodes in (False, True):
                self.storage_exhaustion(self.arguments.failure_fixture, inodes=inodes)
            self.output_limit(self.arguments.failure_fixture)
            assert all(item['qualification_failure'] is None for item in self.results
                       if item['workload']['fixture_id'] == self.arguments.failure_fixture)
        self.web_request('/api/albums/' + self.album_id + '/rename',
                         {'name': 'Survived qualified Film attempts'})
        self.album_name = 'Survived qualified Film attempts'
        self.health()
        command('docker', 'restart', self.web)
        port = json.loads(command('docker', 'inspect', self.web))[0]['NetworkSettings']['Ports']['3000/tcp'][0]['HostPort']
        self.url = 'http://127.0.0.1:' + port

        def resumed():
            try:
                self.health()
                return True
            except OSError:
                return False

        await_condition(resumed)
        (self.output / 'web-observations.json').write_text(json.dumps(self.web_observations, indent=2))
        assert all(digest(Path(path).read_bytes()) == expected for path, expected in self.sources.items())
        identity = dict(instance=self.instance, launcher_sha256=self.launcher_sha256,
                        source_sha256=self.sources, worker_image=self.arguments.worker_image,
                        web_image=self.arguments.web_image, memory_bytes=self.config['memory_bytes'],
                        catalogue=self.config['catalogue_sha256'], envelope=self.config['envelope_sha256'],
                        expected_outcome=self.arguments.expect_outcome,
                        expected_error=self.arguments.expect_error,
                        expected_qualification_failure=self.arguments.expect_qualification_failure)
        (self.output / 'identity.json').write_text(json.dumps(identity, indent=2))
        print(json.dumps(dict(status='passed', attempts=len(self.results), refusals=len(self.refusals))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['launcher', 'worker-image', 'web-image', 'output', 'catalogue', 'envelope']:
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--fixtures')
    parser.add_argument('--memory-gib', type=int, choices=[8, 12, 16, 24, 32], required=True)
    parser.add_argument('--fixture', action='append', required=True)
    parser.add_argument('--expect-outcome', choices=['completed', 'oom', 'allocation-failed',
                        'engine-failed', 'storage-full', 'deadline'], default='completed')
    parser.add_argument('--expect-error', choices=['resource-budget', 'outside-envelope',
                        'unqualified-envelope', 'unknown-fixture'])
    parser.add_argument('--expect-qualification-failure', choices=['peak-exceeded',
                        'processing-oom', 'allocation-failed'])
    parser.add_argument('--alternate-envelope', help='Explicitly reviewed B document for withdrawal A/B/A proof')
    parser.add_argument('--mismatched-envelope', help='Explicit environment-mismatch fault document for replay proof')
    parser.add_argument('--recovery-fixture')
    parser.add_argument('--lifecycle-fixture')
    parser.add_argument('--failure-fixture')
    arguments = parser.parse_args()
    assert os.geteuid() == 0, 'run this isolated kernel verifier with sudo'
    assert all(FILM.IMAGE.fullmatch(value) for value in [arguments.worker_image, arguments.web_image])
    catalogue_data, catalogue = FILM.document(arguments.catalogue, 32768)
    envelope_data, envelope = FILM.document(arguments.envelope, 131072)
    assert envelope['catalogue_sha256'] == digest(catalogue_data)
    assert envelope['image'] == arguments.worker_image
    assert envelope['launcher_sha256'] == digest(Path(arguments.launcher).read_bytes())
    assert all(FILM.ID.fullmatch(value) for value in arguments.fixture)
    fixtures = {item['id']: item for item in catalogue['fixtures']}
    assert len(fixtures) == len(catalogue['fixtures']) and 0 < len(fixtures) <= 16
    if arguments.expect_error != 'unknown-fixture':
        assert all(value in fixtures for value in arguments.fixture)
    if arguments.expect_error == 'resource-budget':
        cases = {item['fixture_id']: item for item in envelope['cases']}
        for fixture_id in arguments.fixture:
            case = cases[fixture_id]
            assert case['status'] == 'qualified'
            # Known K cannot lower this independently evaluated positive bound.
            required_floor = checked_requirement(0, case['empirical_ceiling_bytes'],
                                                  fixtures[fixture_id]['source'].get('bytes', 0),
                                                  case['safety_reserve_bytes'])
            assert required_floor > arguments.memory_gib * FILM.GIB
    if arguments.expect_error:
        assert not any([arguments.expect_qualification_failure, arguments.alternate_envelope,
                        arguments.mismatched_envelope,
                        arguments.recovery_fixture, arguments.lifecycle_fixture, arguments.failure_fixture])
    if arguments.alternate_envelope:
        assert arguments.expect_qualification_failure is not None and len(arguments.fixture) == 1
    for value in [arguments.recovery_fixture, arguments.lifecycle_fixture, arguments.failure_fixture]:
        if value is not None:
            assert value in fixtures and fixtures[value]['width'] * fixtures[value]['height'] <= 2_000_000
            assert value not in arguments.fixture
    if arguments.failure_fixture:
        assert fixtures[arguments.failure_fixture]['source']['kind'] == 'development-tiff'
    verifier = QualifiedFilmQualification(arguments, catalogue_data, catalogue, envelope_data, envelope)
    try:
        verifier.prepare()
        verifier.verify()
    finally:
        verifier.cleanup()


if __name__ == '__main__':
    main()
