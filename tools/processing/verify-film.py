#!/usr/bin/env python3
"""Measure registered Film fixtures through the real, root-only host executor.

Inputs are explicit private qualification documents and pre-staged TIFFs, never a
Photo Library. This command does not infer production budgets or publish images.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import select
import stat
import time

from verify import Qualification, await_condition, command


GIB = 1024 ** 3
ID = re.compile(r"[0-9a-f]{32}")
IMAGE = re.compile(r"sha256:[0-9a-f]{64}")


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON field")
        value[key] = item
    return value


def reject_number(_):
    raise ValueError("integer JSON tokens required")


def document(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, 'rb') as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
            raise ValueError("document is not a bounded regular file")
        data = source.read(limit + 1)
        if len(data) > limit:
            raise ValueError("document grew beyond its bound")
    value = json.loads(data, object_pairs_hook=unique_object,
                       parse_float=reject_number, parse_constant=reject_number)
    return data, value


def copy_fixture(source, destination, expected):
    """Prepare a private operator fixture; this is outside measured execution."""
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, 'rb') as reader:
        before = os.fstat(reader.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_nlink != 1
                or before.st_size != expected['bytes']
                or not 0 < before.st_size <= 2 * GIB):
            raise ValueError("fixture must be a single-link, bounded regular file")
        digest = hashlib.sha256()
        remaining = before.st_size
        with destination.open('xb') as writer:
            destination.chmod(0o444)
            while remaining:
                chunk = reader.read(min(65536, remaining))
                if not chunk:
                    raise ValueError("fixture ended early")
                writer.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            if reader.read(1):
                raise ValueError("fixture grew during preparation")
            writer.flush()
            os.fsync(writer.fileno())
            # Evict only this newly owned fixture's clean cache where supported.
            # Planning still reserves its full source bytes regardless of residency.
            os.posix_fadvise(writer.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        after = os.fstat(reader.fileno())
        identity = lambda metadata: (metadata.st_dev, metadata.st_ino, metadata.st_size,
                                     metadata.st_mtime_ns, metadata.st_ctime_ns)
        if identity(before) != identity(after) or digest.hexdigest() != expected['sha256']:
            raise ValueError("fixture changed or differs from its registered digest")


def expected_artifact(fixture, catalogue):
    reference = fixture['reference']
    return dict(input_pixels_sha256=reference['input_pixels_sha256'],
                film_pixels_sha256=reference['film_pixels_sha256'],
                jpeg_sha256=reference['jpeg_sha256'], jpeg_bytes=reference['jpeg_bytes'],
                width=fixture['width'], height=fixture['height'],
                icc_sha256=catalogue['output_icc_sha256'],
                reference_evidence_sha256=reference['evidence_sha256'])


class FilmQualification(Qualification):
    def __init__(self, arguments, catalogue_data, catalogue, model_data):
        super().__init__(arguments)
        self.film_verifier_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        (self.root / 'launcher.log').touch(mode=0o600)
        self.catalogue = catalogue
        self.fixtures = {item['id']: item for item in catalogue['fixtures']}
        self.config.update(version=2, mode='film-measurement', peer_uid=0,
                           memory_bytes=arguments.memory_gib * GIB,
                           catalogue_sha256=hashlib.sha256(catalogue_data).hexdigest(),
                           resource_model_sha256=hashlib.sha256(model_data).hexdigest())
        for name, data in [('catalogue.json', catalogue_data),
                           ('resource-model.json', model_data)]:
            path = self.root / name
            path.write_bytes(data)
            path.chmod(0o400)
        self.save_config()

    def prepare(self):
        directory = self.root / 'fixtures'
        directory.mkdir(mode=0o700)
        for fixture in self.fixtures.values():
            if fixture['source']['kind'] == 'development-tiff':
                if self.arguments.fixtures is None:
                    raise ValueError("TIFF entries require an explicit fixture directory")
                filename = fixture['id'] + '.tif'
                copy_fixture(Path(self.arguments.fixtures) / filename,
                             directory / filename, fixture['source'])

    def request(self, operation, **fields):
        payload = json.dumps(dict(version=2, instance=self.instance, op=operation,
                                  **fields), separators=(',', ':')).encode()
        return self.raw(payload)

    def intent(self, fixture_id):
        capability = self.request('reconcile')['result']
        assert capability['capability'] == 'film-measurement-only', capability
        return dict(incarnation=capability['incarnation'], sequence=capability['next_sequence'],
                    policy=capability['policy'], bundle=capability['bundle'],
                    catalogue=capability['catalogue'], resource_model=capability['resource_model'],
                    workload=dict(kind='film-fixture', fixture_id=fixture_id))

    def terminal(self, intent):
        receipt = self.receipt(intent)
        remaining = max(0, (receipt['deadline_unix_ms'] - int(time.time() * 1000)) / 1000)
        limit = time.monotonic() + remaining + 15
        next_sample = 0
        filename = self.output / ('resources-' + str(intent['sequence']) + '.jsonl')
        with filename.open('x') as evidence:
            while receipt['state'] != 'settled':
                assert receipt['state'] != 'blocked', receipt
                assert time.monotonic() < limit, 'execution/settlement did not finish'
                if time.monotonic() >= next_sample:
                    self.health()
                    group = Path('/sys/fs/cgroup', self.parent,
                                 receipt['runtime']['attempt_unit'])
                    sample = dict(time_unix_ms=int(time.time()*1000), phase=receipt['phase'])
                    for name in ['memory.current', 'memory.peak', 'memory.events',
                                 'memory.events.local', 'memory.stat', 'memory.swap.current',
                                 'cpu.stat', 'io.stat', 'pids.current', 'cgroup.events']:
                        try:
                            sample[name] = (group / name).read_text()
                        except FileNotFoundError:
                            pass
                    evidence.write(json.dumps(sample) + '\n')
                    evidence.flush()
                    next_sample = time.monotonic() + .5
                time.sleep(.1)
                receipt = self.receipt(intent)
        assert receipt['cleanup'] == 'complete', receipt
        assert receipt['evidence']['populated'] is False, receipt
        runtime = receipt['runtime']
        assert not Path('/sys/fs/cgroup', self.parent, runtime['attempt_unit']).exists()
        assert not (self.root / 'attempts' / runtime['launch_id']).exists()
        if runtime['container_id']:
            assert not command('docker', 'ps', '-aq', '--no-trunc',
                               '--filter', 'id=' + runtime['container_id'])
        assert receipt['limits']['memory_bytes'] == self.config['memory_bytes']
        assert receipt['limits']['swap_bytes'] == 0
        if receipt['outcome'] == 'completed':
            fixture = self.fixtures[intent['workload']['fixture_id']]
            expected = expected_artifact(fixture, self.catalogue)
            result = receipt['result']
            assert result['outcome'] == 'completed', receipt
            assert result['artifact'] == expected, (result['artifact'], expected)
            assert result['manifest'] == receipt['manifest']
        self.health()
        self.results.append(receipt)
        (self.output / 'receipts.json').write_text(json.dumps(self.results, indent=2))
        return receipt

    def film_marker(self, intent):
        deadline_ms = self.receipt(intent)['deadline_unix_ms']
        def observed():
            assert self.process.poll() is None, 'launcher exited before barrier'
            path = self.root / 'faults' / 'marker.json'
            return json.loads(path.read_text()) if path.exists() else None
        return await_condition(observed, max(0, (deadline_ms-int(time.time()*1000))/1000))

    def crash_film(self, fixture_id, phase):
        intent = self.intent(fixture_id)
        self.arm(intent, phase)
        accepted = self.request('start', **intent)['result']['receipt']
        marker = self.film_marker(intent)
        assert marker['phase'] == phase
        assert marker['launch_id'] == accepted['runtime']['launch_id']
        before = json.loads((self.root / 'registry.json').read_text())
        self.stop(crash=True)
        self.disarm()
        assert self.start()['incarnation'] == intent['incarnation']
        receipt = self.terminal(intent)
        expected = 'completed' if phase == 'after-validated-result' else 'interrupted'
        assert receipt['outcome'] == expected, receipt
        assert receipt['runtime']['launch_id'] == marker['launch_id']
        assert receipt['deadline_unix_ms'] == accepted['deadline_unix_ms']
        if phase in ['after-intent', 'after-slice', 'after-create-response', 'after-container-bound']:
            assert receipt['phase'] == 'preparing', receipt
            assert receipt['evidence']['exit_code'] is None, receipt
        (self.output / (phase + '.json')).write_text(json.dumps(
            dict(marker=marker, before=before, after=receipt), indent=2))
        self.run(fixture_id, 'completed')

    def cancel_sealed(self, fixture_id):
        intent = self.intent(fixture_id)
        self.arm(intent, 'after-snapshot-sealed')
        self.request('start', **intent)
        marker = self.film_marker(intent)
        receipt = self.request('cancel', incarnation=intent['incarnation'],
                               sequence=intent['sequence'])['result']['receipt']
        assert receipt['cancellation_requested']
        self.release(marker)
        result = self.terminal(intent)
        assert result['outcome'] == 'cancelled', result
        assert result['result'] is None or result['result']['outcome'] != 'completed'
        self.disarm()
        self.run(fixture_id, 'completed')

    def changed_source(self, fixture_id):
        fixture = self.fixtures[fixture_id]
        if fixture['source']['kind'] != 'development-tiff':
            return
        # Mutate only the private copied qualification input, never its source.
        path = self.root / 'fixtures' / (fixture_id + '.tif')
        intent = self.intent(fixture_id)
        self.arm(intent, 'after-stage-release-intent')
        self.request('start', **intent)
        marker = self.film_marker(intent)
        with path.open('r+b') as copy:
            original = copy.read(1)
            assert original
            copy.seek(0)
            copy.write(bytes([original[0] ^ 1]))
            copy.flush()
            os.fsync(copy.fileno())
            try:
                self.release(marker)
                receipt = self.terminal(intent)
                assert receipt['outcome'] == 'engine-failed', receipt
                assert receipt['detail'] == 'source-mismatch', receipt
            finally:
                copy.seek(0)
                copy.write(original)
                copy.flush()
                os.fsync(copy.fileno())
        self.disarm()
        self.run(fixture_id, 'completed')

    def storage_exhaustion(self, fixture_id, *, inodes=False):
        intent = self.intent(fixture_id)
        self.arm(intent, 'after-snapshot-sealed')
        accepted = self.request('start', **intent)['result']['receipt']
        marker = self.film_marker(intent)
        storage = self.root / 'attempts' / marker['launch_id'] / 'work'
        assert accepted['runtime']['launch_id'] == marker['launch_id']
        registry = json.loads((self.root / 'registry.json').read_text())
        record = registry['records'][str(intent['sequence'])]
        before = os.statvfs(storage)
        assert before.f_blocks * before.f_frsize == 4 * GIB
        assert before.f_files == 4096
        # Only this private capped tmpfs is filled. Its host-precharged pages
        # establish storage exhaustion, not an attempt-memory measurement.
        with (storage / 'native' / 'qualification-storage-fill').open('xb') as filler:
            metadata = os.fstat(filler.fileno())
            mount = next(line for line in Path('/proc/self/fdinfo', str(filler.fileno())).read_text().splitlines()
                         if line.startswith('mnt_id:'))
            assert int(mount.split()[1]) == record['mount_id']
            remaining = os.fstatvfs(filler.fileno())
            if inodes:
                assert 0 < remaining.f_favail < 4096
                for index in range(remaining.f_favail):
                    with (storage / 'native' / ('qualification-inode-' + str(index))).open('xb'):
                        pass
                assert os.fstatvfs(filler.fileno()).f_favail == 0
            else:
                os.posix_fallocate(filler.fileno(), 0, remaining.f_bavail * before.f_frsize)
                os.fsync(filler.fileno())
                assert os.fstatvfs(filler.fileno()).f_bavail == 0
            after = os.fstatvfs(filler.fileno())
            evidence = dict(kind='host-injected-tmpfs-inode-exhaustion' if inodes else 'host-injected-tmpfs-exhaustion',
                            included_in_memory_qualification=False,
                            launch_id=marker['launch_id'], mount_id=record['mount_id'],
                            device=metadata.st_dev, inode=metadata.st_ino,
                            filled_bytes=0 if inodes else remaining.f_bavail * before.f_frsize,
                            filled_inodes=remaining.f_favail + 1 if inodes else 1,
                            capacity_before=dict(bytes_available=before.f_bavail * before.f_frsize,
                                                 inodes_available=before.f_favail),
                            capacity_after=dict(bytes_available=after.f_bavail * after.f_frsize,
                                                inodes_available=after.f_favail))
        self.release(marker)
        receipt = self.terminal(intent)
        evidence['receipt'] = receipt
        filename = 'inode-exhaustion.json' if inodes else 'storage-exhaustion.json'
        (self.output / filename).write_text(json.dumps(evidence, indent=2))
        assert receipt['outcome'] == 'storage-full', receipt
        assert receipt['result'] is None or receipt['result']['outcome'] != 'completed'
        if inodes:
            assert receipt['result'] is not None, 'native failure record was lost'
            assert receipt['result']['outcome'] == 'storage-full'
            assert receipt['result']['detail'] is None and receipt['result']['phase'] == 'engine'
            assert evidence['capacity_after']['bytes_available'] > 0
        self.disarm()
        self.run(fixture_id, 'completed')

    def retained_writer(self, fixture_id):
        assert self.fixtures[fixture_id]['source']['kind'] == 'development-tiff'
        intent = self.intent(fixture_id)
        fault = self.root / 'faults' / 'retain-snapshot-writer.json'
        fault.write_text(json.dumps({key: intent[key] for key in ['incarnation', 'sequence']}))
        fault.chmod(0o600)
        self.arm(intent, 'after-stage-ack')
        self.request('start', **intent)
        marker = self.film_marker(intent)
        registry = json.loads((self.root / 'registry.json').read_text())
        record = registry['records'][str(intent['sequence'])]
        snapshot = record['film']['snapshot']
        observed = []
        process = Path('/proc', str(self.process.pid))
        for path in (process / 'fd').iterdir():
            try:
                metadata = path.stat()
            except FileNotFoundError:
                continue
            if (metadata.st_dev, metadata.st_ino) == (snapshot['device'], snapshot['inode']):
                info = (process / 'fdinfo' / path.name).read_text()
                flags = int(next(line.split()[1] for line in info.splitlines()
                                 if line.startswith('flags:')), 8)
                if flags & os.O_ACCMODE != os.O_RDONLY:
                    observed.append(dict(fd=int(path.name), fdinfo=info))
        assert observed, 'fixed fault did not retain the actual snapshot writer'
        self.release(marker)
        receipt = self.terminal(intent)
        after = json.loads((self.root / 'registry.json').read_text())['records'][str(intent['sequence'])]
        evidence = dict(snapshot=snapshot, observed_writers=observed, receipt=receipt,
                        engine_release_intent=after['film']['engine_release_intent'])
        (self.output / 'retained-writer.json').write_text(json.dumps(evidence, indent=2))
        assert receipt['outcome'] == 'engine-failed' and receipt['detail'] == 'source-mismatch', receipt
        assert not after['film']['engine_release_intent']
        assert receipt['result'] is None
        fault.unlink(missing_ok=True)
        self.disarm()
        self.stop()
        assert self.start()['incarnation'] == intent['incarnation']
        assert self.receipt(intent) == receipt, 'restart changed a settled sealing failure'
        self.run(fixture_id, 'completed')

    def output_limit(self, fixture_id):
        intent = self.intent(fixture_id)
        self.request('start', **intent)
        def released():
            receipt = self.receipt(intent)
            assert receipt['state'] not in ('blocked', 'settled'), receipt
            return receipt if receipt['phase'] == 'engine' else None
        receipt = await_condition(released)
        container = receipt['runtime']['container_id']
        assert re.fullmatch(r'[0-9a-f]{64}', container)
        group = Path('/sys/fs/cgroup', self.parent, receipt['runtime']['attempt_unit'],
                     'docker-' + container + '.scope')
        leaf = group / 'workload'
        command('docker', 'pause', container)
        try:
            assert 'frozen 1' in (group / 'cgroup.events').read_text().splitlines()
            pids = [int(value) for value in (leaf / 'cgroup.procs').read_text().split()]
            assert 1 <= len(pids) <= 256
            candidates = []
            for pid in pids:
                process = Path('/proc', str(pid))
                arguments = (process / 'cmdline').read_bytes().split(b'\0')
                if arguments[:2] == [b'/opt/runtime/bin/python', b'/opt/film-measurement/adapter.py']:
                    candidates.append(pid)
            assert len(candidates) == 1, candidates
            pid = candidates[0]
            descriptor = os.pidfd_open(pid)
            try:
                poll = select.poll()
                poll.register(descriptor, select.POLLIN)
                assert not poll.poll(0), 'child exited before fault injection'
                process = Path('/proc', str(pid))
                identity = (process / 'stat').read_text()
                assert (process / 'cgroup').read_text().strip() == '0::/' + str(leaf.relative_to('/sys/fs/cgroup'))
                original = resource.prlimit(pid, resource.RLIMIT_FSIZE)
                assert original == (512 * 1024 * 1024,) * 2, original
                assert resource.prlimit(pid, resource.RLIMIT_FSIZE, (4096, 4096)) == original
                assert resource.prlimit(pid, resource.RLIMIT_FSIZE) == (4096, 4096)
                assert (process / 'stat').read_text().split(') ', 1)[1].split()[19] == identity.split(') ', 1)[1].split()[19]
                assert not poll.poll(0), 'child exited while frozen'
                evidence = dict(kind='operator-lowered-child-file-limit', launch_id=receipt['runtime']['launch_id'],
                                container=container, pid=pid, process_stat=identity,
                                original_limit=list(original), injected_limit=[4096, 4096],
                                natural_512_mib_exhaustion=False)
            finally:
                os.close(descriptor)
        finally:
            command('docker', 'unpause', container)
        receipt = self.terminal(intent)
        evidence['receipt'] = receipt
        (self.output / 'output-limit.json').write_text(json.dumps(evidence, indent=2))
        assert receipt['outcome'] == 'storage-full' and receipt['detail'] == 'output-limit', receipt
        # EFBIG and SIGXFSZ share this outcome; this evidence makes no unsupported
        # claim about which of the two the child observed.
        self.run(fixture_id, 'completed')

    def verify(self):
        self.start()
        self.start_web()
        for fixture_id in self.arguments.fixture or self.fixtures:
            receipt = self.run(fixture_id, self.arguments.expect_outcome)
            late = self.request('cancel', incarnation=receipt['incarnation'],
                                sequence=receipt['sequence'])['result']['receipt']
            assert late == receipt, 'settled outcome changed after late cancellation'
            if self.arguments.recovery_fixture:
                self.run(self.arguments.recovery_fixture, 'completed')
        if self.arguments.lifecycle_fixture:
            fixture_id = self.arguments.lifecycle_fixture
            for phase in ['after-intent', 'after-slice', 'after-create-response', 'after-container-bound',
                          'after-stage-release-intent', 'after-stage-ack',
                          'after-snapshot-sealed', 'after-engine-release-intent',
                          'after-validated-result']:
                self.crash_film(fixture_id, phase)
            self.cancel_sealed(fixture_id)
            self.changed_source(fixture_id)
        if self.arguments.failure_fixture:
            self.retained_writer(self.arguments.failure_fixture)
            self.storage_exhaustion(self.arguments.failure_fixture)
            self.storage_exhaustion(self.arguments.failure_fixture, inodes=True)
            self.output_limit(self.arguments.failure_fixture)
        self.web_request('/api/albums/' + self.album_id + '/rename',
                         {'name': 'Survived Film measurements'})
        self.album_name = 'Survived Film measurements'
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
        assert hashlib.sha256(Path(__file__).read_bytes()).hexdigest() == self.film_verifier_sha256, 'verifier source changed during qualification'
        identity = dict(instance=self.instance, launcher_sha256=self.launcher_sha256,
                        verifier_sha256=self.film_verifier_sha256,
                        native_verifier_sha256=self.verifier_sha256,
                        worker_image=self.arguments.worker_image, web_image=self.arguments.web_image,
                        memory_bytes=self.config['memory_bytes'],
                        catalogue=self.config['catalogue_sha256'],
                        resource_model=self.config['resource_model_sha256'],
                        host=command('uname', '-r'),
                        docker=json.loads(command('docker', 'info', '--format', '{{json .}}'))['CgroupDriver'])
        (self.output / 'identity.json').write_text(json.dumps(identity, indent=2))
        print(json.dumps(dict(status='passed', attempts=len(self.results), evidence=str(self.output))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['launcher', 'worker-image', 'web-image', 'output', 'catalogue', 'resource-model']:
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--fixtures', help='Private pre-staged TIFF directory, keyed by fixture ID')
    parser.add_argument('--memory-gib', type=int, choices=[8, 12, 16, 24, 32], required=True)
    parser.add_argument('--fixture', action='append', help='Registered fixture ID; default: every entry')
    parser.add_argument('--expect-outcome', choices=['completed', 'oom', 'allocation-failed',
                        'engine-failed', 'storage-full', 'deadline'], default='completed')
    parser.add_argument('--lifecycle-fixture', help='Registered small fixture for crash/cancel checks')
    parser.add_argument('--failure-fixture', help='Registered small fixture for injected storage/file-limit failures')
    parser.add_argument('--recovery-fixture', help='Different small fixture to run after each explicitly expected failure')
    arguments = parser.parse_args()
    assert os.geteuid() == 0, 'run this isolated kernel verifier with sudo'
    assert all(IMAGE.fullmatch(value) for value in [arguments.worker_image, arguments.web_image])
    catalogue_data, catalogue = document(arguments.catalogue, 32 * 1024)
    model_data, _ = document(arguments.resource_model, 128 * 1024)
    fixtures = catalogue['fixtures']
    ids = [entry['id'] for entry in fixtures]
    assert 1 <= len(ids) <= 16 and len(set(ids)) == len(ids)
    assert all(ID.fullmatch(value) for value in ids)
    assert all(value in ids for value in arguments.fixture or [])
    assert arguments.lifecycle_fixture is None or arguments.lifecycle_fixture in ids
    assert arguments.failure_fixture is None or arguments.failure_fixture in ids
    assert arguments.recovery_fixture is None or arguments.recovery_fixture in ids
    if arguments.recovery_fixture:
        assert arguments.expect_outcome != 'completed'
        assert arguments.fixture and arguments.recovery_fixture not in arguments.fixture
        fixture = next(item for item in fixtures if item['id'] == arguments.recovery_fixture)
        assert fixture['width'] * fixture['height'] <= 2_000_000
    for fixture_id in [arguments.lifecycle_fixture, arguments.failure_fixture]:
        if fixture_id is None:
            continue
        fixture = next(item for item in fixtures if item['id'] == fixture_id)
        assert fixture['width'] * fixture['height'] <= 2_000_000
        assert arguments.expect_outcome == 'completed'
        if fixture_id == arguments.failure_fixture:
            assert fixture['source']['kind'] == 'development-tiff'
    verifier = FilmQualification(arguments, catalogue_data, catalogue, model_data)
    try:
        verifier.prepare()
        verifier.verify()
    finally:
        verifier.cleanup()


if __name__ == '__main__':
    main()
