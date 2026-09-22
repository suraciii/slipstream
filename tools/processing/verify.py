#!/usr/bin/env python3
"""Run the actual qualification launcher against private synthetic kernel fixtures.

Requires root, systemd, cgroup v2, Docker's systemd driver, and local pinned images.
Never reads a Photo Library. Evidence is private; no uploads are performed.
"""
import argparse
import array
import concurrent.futures
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import time
import urllib.request
import uuid


UID_CLIENT = r"""
import json, os, pathlib, socket, struct, sys
payload=bytes.fromhex(sys.argv[2])
with socket.socket(socket.AF_UNIX) as stream:
    stream.settimeout(2);stream.connect(sys.argv[1]);stream.sendall(struct.pack('!I',len(payload))+payload)
    def receive(count):
        data=b''
        while len(data)<count:
            chunk=stream.recv(count-len(data))
            if not chunk:raise RuntimeError('short frame')
            data+=chunk
        return data
    length,=struct.unpack('!I',receive(4))
    assert 0<length<=65536
    response=json.loads(receive(length))
print(json.dumps(dict(response=response,uid=os.getuid(),cgroup=pathlib.Path('/proc/self/cgroup').read_text())))
"""

def command(*arguments, check=True, timeout=15):
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    if check and result.returncode:
        raise AssertionError((arguments, result.returncode, result.stderr[-4096:]))
    return result.stdout.strip()


def await_condition(probe, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = probe()
        if result:
            return result
        time.sleep(0.025)
    raise AssertionError("condition did not become true within its fixed deadline")


def attempt_absent(parent, unit, deadline):
    def present(path):
        try:
            path.lstat()
            return True
        except FileNotFoundError:
            return False

    def remaining():
        value = deadline - time.monotonic()
        assert value > 0, 'attempt absence observation expired'
        return value
    loaded = command('systemctl', '--system', 'list-units', '--all', '--plain',
                     '--no-legend', '--no-pager', unit, timeout=remaining())
    directories = command('systemctl', '--system', 'show', '--property=UnitPath', '--value',
                          timeout=remaining()).split()
    assert directories and '/run/systemd/transient' in directories
    missing = not loaded and not present(Path('/sys/fs/cgroup', parent, unit))
    for directory in directories:
        assert Path(directory).is_absolute()
        for name in [unit, unit + '.d']:
            path = Path(directory, name)
            missing = not present(path) and missing
    remaining()
    return missing


def assert_attempt_absent(parent, unit):
    assert attempt_absent(parent, unit, time.monotonic() + 5)


def wait_attempt_absent(parent, unit):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if attempt_absent(parent, unit, deadline):
            return
        time.sleep(min(.025, max(0, deadline - time.monotonic())))
    raise AssertionError('attempt did not unload within its fixed observation deadline')


class Qualification:
    def __init__(self, arguments):
        self.arguments = arguments
        self.instance = uuid.uuid4().hex
        self.root = Path('/var/lib/slipstream-processing-qualification') / self.instance
        self.root.mkdir(parents=True, mode=0o700)
        self.launcher = self.root / 'launcher'
        shutil.copyfile(arguments.launcher, self.launcher)
        self.launcher.chmod(0o700)
        self.launcher_sha256 = hashlib.sha256(self.launcher.read_bytes()).hexdigest()
        self.verifier_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        self.parent = f'slipstreamprocessing{self.instance}.slice'
        self.socket = self.root / 'control.sock'
        self.process = None
        self.web = None
        self.results = []
        self.web_observations = []
        self.permitted_peer_observations = []
        self.album_name = None
        self.output = Path(arguments.output).resolve()
        self.output.mkdir(parents=True, exist_ok=False)
        self.config = dict(version=1, mode='qualification', instance=self.instance,
                           root=str(self.root), socket=str(self.socket), peer_uid=0,
                           image=arguments.worker_image, memory_bytes=128*1024*1024,
                           receipt_retention_seconds=86400)
        self.save_config()

    def save_config(self):
        path = self.root / 'config.json'
        path.write_text(json.dumps(self.config))
        path.chmod(0o600)

    def start(self):
        assert self.process is None
        self.log = (self.root / 'launcher.log').open('ab')
        self.process = subprocess.Popen([str(self.launcher), '--config', str(self.root/'config.json')],
                                        stdout=self.log, stderr=subprocess.STDOUT)
        def available():
            assert self.process.poll() is None, 'launcher exited'
            try:
                result = self.request('reconcile').get('result')
                return result if result and result['availability'] == 'available' else None
            except (FileNotFoundError, ConnectionRefusedError):
                return None
        self.capability = await_condition(available)
        return self.capability

    def stop(self, crash=False):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.kill() if crash else self.process.terminate()
            self.process.wait(timeout=10)
            self.process = None
            self.log.close()

    def request(self, operation, **fields):
        payload = json.dumps(dict(version=1, instance=self.instance, op=operation, **fields)).encode()
        return self.raw(payload)

    def raw(self, payload):
        if self.config['peer_uid']==1000:
            result=subprocess.run(['/usr/bin/setpriv','--reuid=1000','--regid=1000','--clear-groups','/usr/bin/python3','-c',UID_CLIENT,str(self.socket),payload.hex()],capture_output=True,text=True,timeout=3)
            if result.returncode:
                if 'ConnectionRefusedError' in result.stderr or 'FileNotFoundError' in result.stderr or 'PermissionError' in result.stderr:
                    raise ConnectionRefusedError()
                raise AssertionError(result.stderr)
            observed=json.loads(result.stdout)
            assert observed['uid']==1000 and self.parent not in observed['cgroup']
            self.permitted_peer_observations.append({key:observed[key] for key in ['uid','cgroup']})
            return observed['response']
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(3)
            stream.connect(str(self.socket))
            stream.sendall(struct.pack('!I', len(payload)) + payload)
            def receive(count):
                data = b''
                while len(data) < count:
                    chunk = stream.recv(count-len(data))
                    assert chunk, 'response closed before complete frame'
                    data += chunk
                return data
            length, = struct.unpack('!I', receive(4))
            assert 0 < length <= 65536
            return json.loads(receive(length))

    def intent(self, workload):
        cap = self.request('reconcile')['result']
        return dict(incarnation=cap['incarnation'], sequence=cap['next_sequence'],
                    policy=cap['policy'], bundle=cap['bundle'], workload=workload)

    def receipt(self, intent):
        result = self.request('inspect', incarnation=intent['incarnation'], sequence=intent['sequence'])
        assert 'result' in result, result
        return result['result']['receipt']

    def terminal(self, intent):
        def settled():
            self.health()
            receipt = self.receipt(intent)
            assert receipt['state'] != 'blocked', receipt
            return receipt if receipt['state'] == 'settled' else None
        receipt = await_condition(settled, 35)
        assert receipt['cleanup'] == 'complete'
        assert receipt['evidence']['populated'] is False
        runtime = receipt['runtime']
        assert_attempt_absent(self.parent, runtime['attempt_unit'])
        assert not (self.root/'attempts'/runtime['launch_id']).exists()
        if runtime['container_id']:
            assert not command('docker', 'ps', '-aq', '--no-trunc', '--filter', 'id='+runtime['container_id'])
        self.health()
        self.results.append(receipt)
        return receipt

    def blocked_recovery(self, intent):
        def recovered():
            try:
                capability = self.request('reconcile')['result']
                receipt = capability['active']
                if receipt is None:
                    return None
                assert (receipt['incarnation'], receipt['sequence']) == (intent['incarnation'], intent['sequence'])
                if (capability['availability'] == 'blocked' and
                        receipt['state'] == 'blocked' and receipt['cleanup'] == 'uncertain'):
                    return receipt
                return None
            except (FileNotFoundError, ConnectionRefusedError):
                return None
        return await_condition(recovered)

    def run(self, workload, outcome):
        intent = self.intent(workload)
        response = self.request('start', **intent)
        assert 'result' in response, response
        accepted = response['result']['receipt']
        replay = self.request('start', **intent)['result']['receipt']
        assert replay['runtime']['launch_id'] == accepted['runtime']['launch_id']
        result = self.terminal(intent)
        assert result['outcome'] == outcome, result
        return result

    def arm(self, intent, phase):
        path = self.root/'faults'/'arm.json'
        path.write_text(json.dumps(dict(phase=phase, incarnation=intent['incarnation'], sequence=intent['sequence'])))
        path.chmod(0o600)

    def marker(self):
        path = self.root/'faults'/'marker.json'
        return await_condition(lambda: json.loads(path.read_text()) if path.exists() else None)

    def disarm(self):
        for name in ['arm.json', 'marker.json', 'release.json']:
            (self.root/'faults'/name).unlink(missing_ok=True)

    def release(self, marker):
        path = self.root/'faults'/'release.json'
        path.write_text(json.dumps(marker))
        path.chmod(0o600)

    def crash(self, phase, workload='probe-success'):
        intent = self.intent(workload)
        self.arm(intent, phase)
        response = self.request('start', **intent)
        assert 'result' in response, response
        accepted = response['result']['receipt']
        marker = self.marker()
        assert marker['phase'] == phase and marker['launch_id'] == accepted['runtime']['launch_id']
        record = json.loads((self.root/'registry.json').read_text())['records'][str(intent['sequence'])]
        evidence = dict(phase=phase, marker=marker, before=record)
        if phase == 'after-exit':
            runtime = record['receipt']['runtime']
            path = Path('/sys/fs/cgroup', self.parent, runtime['attempt_unit'])
            assert not list(path.glob('docker-*.scope'))
            evidence['retained_before_restart'] = {name:(path/name).read_text() for name in ['memory.peak','memory.events','memory.events.local','cgroup.events','memory.current']}
        self.stop(crash=True)
        self.disarm()
        cap = self.start()
        assert cap['incarnation'] == intent['incarnation']
        receipt = self.terminal(intent)
        assert receipt['runtime']['launch_id'] == marker['launch_id']
        assert receipt['deadline_unix_ms'] == accepted['deadline_unix_ms']
        if phase in ['after-exit', 'after-evidence', 'after-container-removal','after-storage-unmount','after-slice-stop']:
            assert receipt['outcome'] == ('oom' if workload == 'probe-descendant-oom' else 'storage-full' if workload == 'probe-storage-full' else 'engine-failed' if workload == 'probe-exit-137' else 'completed'), receipt
        else:
            assert receipt['outcome'] == 'interrupted', receipt
        evidence['after'] = receipt
        (self.output/(phase+'-'+workload+'-'+str(intent['sequence'])+'.json')).write_text(json.dumps(evidence, indent=2))
        self.run('probe-success', 'completed')

    def pending_slice_stop(self):
        arguments = argparse.Namespace(**vars(self.arguments))
        arguments.output = str(self.output / 'pending-slice-stop')
        case = Qualification(arguments)
        try:
            case.start()
            intent = case.intent('probe-success')
            case.arm(intent, 'after-slice-stop-intent')
            case.request('start', **intent)
            marker = case.marker()
            before = json.loads((case.root/'registry.json').read_text())['records'][str(intent['sequence'])]
            assert before['manager_pending'] == 'slice-stop' and not before['stop_confirmed']
            assert before['receipt']['outcome'] == 'completed'
            unit = before['receipt']['runtime']['attempt_unit']
            group = Path('/sys/fs/cgroup', case.parent, unit)
            inode = group.stat().st_ino
            case.stop(crash=True); case.disarm()
            case.log = (case.root/'launcher.log').open('ab')
            case.process = subprocess.Popen([str(case.launcher),'--config',str(case.root/'config.json')],stdout=case.log,stderr=subprocess.STDOUT)
            case.blocked_recovery(intent)
            after = json.loads((case.root/'registry.json').read_text())['records'][str(intent['sequence'])]
            assert after['manager_pending'] == 'slice-stop' and not after['stop_confirmed']
            assert after['receipt']['outcome'] == 'completed'
            assert after['receipt']['cleanup'] == 'uncertain'
            assert group.stat().st_ino == inode
            assert command('systemctl','show',unit,'--property=InvocationID','--value') == before['unit_invocation']
            (case.output/'blocked-proof.json').write_text(json.dumps(dict(marker=marker,before=before,after=after),indent=2))
        finally:
            # Explicit verifier-owned cleanup is separate from the blocked receipt.
            case.cleanup()

    def foreign_configuration_after_stop(self):
        for symbolic in [False, True]:
            arguments = argparse.Namespace(**vars(self.arguments))
            arguments.output = str(self.output / ('foreign-after-stop-' + str(symbolic).lower()))
            case = Qualification(arguments)
            directory = None
            created_inode = None
            try:
                case.start()
                intent = case.intent('probe-success')
                case.arm(intent, 'after-slice-stop')
                case.request('start', **intent)
                marker = case.marker()
                before = json.loads((case.root/'registry.json').read_text())['records'][str(intent['sequence'])]
                assert before['stop_confirmed'] and before['manager_pending'] is None
                unit = before['receipt']['runtime']['attempt_unit']
                wait_attempt_absent(case.parent, unit)
                case.stop(crash=True); case.disarm()
                directory = Path('/run/systemd/system.control', unit + '.d')
                if symbolic:
                    directory.symlink_to('unavailable-foreign-target')
                else:
                    directory.mkdir()
                created_inode = directory.lstat().st_ino
                if not symbolic:
                    (directory/'foreign.conf').write_text('[Slice]\nMemoryMax=67108864\n')
                original = directory.lstat()
                case.log = (case.root/'launcher.log').open('ab')
                case.process = subprocess.Popen([str(case.launcher),'--config',str(case.root/'config.json')],stdout=case.log,stderr=subprocess.STDOUT)
                case.blocked_recovery(intent)
                after = json.loads((case.root/'registry.json').read_text())['records'][str(intent['sequence'])]
                assert after['receipt']['outcome'] == 'completed' and after['receipt']['cleanup'] == 'uncertain'
                assert after['stop_confirmed'] and after['manager_pending'] is None
                assert not Path('/sys/fs/cgroup',case.parent,unit).exists()
                assert directory.lstat().st_ino == original.st_ino
                if symbolic:
                    assert directory.readlink() == Path('unavailable-foreign-target')
                else:
                    assert (directory/'foreign.conf').read_text() == '[Slice]\nMemoryMax=67108864\n'
                (case.output/'blocked-proof.json').write_text(json.dumps(dict(marker=marker,before=before,after=after,symbolic=symbolic),indent=2))
            finally:
                case.stop()
                # Remove only the exact foreign fixture created by this verifier.
                if created_inode is not None:
                    assert directory.lstat().st_ino == created_inode
                    if directory.is_symlink(): directory.unlink()
                    elif directory.exists():
                        (directory/'foreign.conf').unlink(missing_ok=True); directory.rmdir()
                case.cleanup()

    def web_request(self, path, body=None):
        request=urllib.request.Request(self.url+path, data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type':'application/json','Origin':self.url})
        with urllib.request.urlopen(request,timeout=2) as response:
            assert response.status==200
            return json.load(response)

    def health(self):
        if self.web:
            assert self.web_request('/healthz')['status']=='ok'
            overview=self.web_request('/api/overview')
            assert overview['photoCount']==0
            if self.album_name:
                assert overview['published'] and len(overview['albums'])==1
                assert overview['albums'][0]['id']==self.album_id and overview['albums'][0]['name']==self.album_name
            self.web_observations.append(dict(time_unix_ms=int(time.time()*1000),published=overview['published'],photo_count=overview['photoCount'],albums=overview['albums']))
            return overview

    def start_web(self):
        library = self.root/'empty-library'; state = self.root/'web-state'; cache = self.root/'web-cache'
        for path in [library,state,cache]:
            path.mkdir(); os.chown(path,1000,1000)
        self.web = command('docker','create','--name','slipstream-qualification-web-'+self.instance,
            '--user','1000:1000','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges:true',
            '--log-driver','none','--memory','512m','--memory-swap','512m','--pids-limit','64',
            '-p','127.0.0.1::3000','-e','SLIPSTREAM_LIBRARY_ROOT=/library','-e','SLIPSTREAM_STATE_DIRECTORY=/state',
            '-e','SLIPSTREAM_CACHE_DIRECTORY=/cache','-e','SLIPSTREAM_HOST=0.0.0.0','-e','SLIPSTREAM_PORT=3000',
            '--mount',f'type=bind,source={library},target=/library,readonly',
            '--mount',f'type=bind,source={state},target=/state','--mount',f'type=bind,source={cache},target=/cache',
            self.arguments.web_image)
        command('docker','start',self.web)
        port=json.loads(command('docker','inspect',self.web))[0]['NetworkSettings']['Ports']['3000/tcp'][0]['HostPort']
        self.url='http://127.0.0.1:'+port
        def ready():
            try:return self.health()['published']
            except (OSError,urllib.error.URLError):return False
        await_condition(ready)
        self.web_request('/api/albums',{'name':'Qualification album'})
        overview=self.web_request('/api/overview')
        assert len(overview['albums'])==1
        self.album_id=overview['albums'][0]['id'];self.album_name='Qualification album'
        self.health()
        pid=json.loads(command('docker','inspect',self.web))[0]['State']['Pid']
        assert self.parent not in Path(f'/proc/{pid}/cgroup').read_text()
        assert self.parent not in Path(f'/proc/{self.process.pid}/cgroup').read_text()

    def control_contract(self):
        capability=self.request('reconcile')['result']
        sequence=capability['next_sequence']
        base=dict(version=1,instance=self.instance,op='inspect',incarnation=capability['incarnation'],sequence=sequence)
        for bad in [dict(base,sequence=-1),dict(base,sequence=1.0),dict(base,sequence=True),dict(base,sequence=2**64),dict(base,argv=['sh'])]:
            assert self.raw(json.dumps(bad).encode())['error']['code']=='invalid-request'
        raw=json.dumps(base).encode()
        assert self.raw(raw+b'{}')['error']['code']=='invalid-request'
        assert self.raw(raw.replace(b'"version": 1',b'"version": 1, "version": 1'))['error']['code']=='invalid-request'
        descriptor_count=len(list(Path(f'/proc/{self.process.pid}/fd').iterdir()))
        with open('/dev/null','rb') as file:
            for _ in range(16):
                with socket.socket(socket.AF_UNIX) as stream:
                    stream.settimeout(3);stream.connect(str(self.socket))
                    stream.sendmsg([struct.pack('!I',len(raw))+raw],[(socket.SOL_SOCKET,socket.SCM_RIGHTS,array.array('i',[file.fileno()]))])
                    length=stream.recv(4)
                    assert len(length)==4
                    response=json.loads(stream.recv(struct.unpack('!I',length)[0]))
                    assert response['error']['code']=='invalid-request'
        await_condition(lambda:len(list(Path(f'/proc/{self.process.pid}/fd').iterdir()))==descriptor_count)
        # Authenticate a foreign UID even when an operator temporarily allows its connect.
        self.root.chmod(0o701);self.socket.chmod(0o666)
        code="import socket,struct,json,sys;s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1]);s.sendall(b'\\0\\0\\0\\1x');h=s.recv(4);r=json.loads(s.recv(struct.unpack('!I',h)[0]));assert r['error']['code']=='unauthorized'"
        result=subprocess.run(['/usr/bin/python3','-c',code,str(self.socket)],capture_output=True,text=True,timeout=3,preexec_fn=lambda:(os.setgroups([]),os.setgid(1000),os.setuid(1000)))
        self.root.chmod(0o700);self.socket.chmod(0o600)
        assert result.returncode==0,result.stderr
        # Four stalled readers own the four bounded slots; an excess peer is rejected.
        threads=len(list(Path(f'/proc/{self.process.pid}/task').iterdir()))
        streams=[];started=time.monotonic()
        try:
            for _ in range(4):
                stream=socket.socket(socket.AF_UNIX);stream.settimeout(3);stream.connect(str(self.socket));stream.sendall(b'\0');streams.append(stream)
            await_condition(lambda:len(list(Path(f'/proc/{self.process.pid}/task').iterdir()))==threads+4)
            with socket.socket(socket.AF_UNIX) as extra:
                extra.settimeout(1);extra.connect(str(self.socket));assert extra.recv(1)==b''
            for stream in streams:
                header=stream.recv(4);assert len(header)==4
                response=json.loads(stream.recv(struct.unpack('!I',header)[0]));assert response['error']['code']=='invalid-request'
            assert time.monotonic()-started<3
        finally:
            for stream in streams:stream.close()
        await_condition(lambda:len(list(Path(f'/proc/{self.process.pid}/task').iterdir()))==threads)
        assert self.request('reconcile')['result']['next_sequence']==sequence
        # An exact duplicate and a lost response retain one durable launch identity.
        intent=self.intent('probe-hold')
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            replies=list(pool.map(lambda _:self.request('start',**intent),range(2)))
        launches={reply['result']['receipt']['runtime']['launch_id'] for reply in replies};assert len(launches)==1
        assert self.request('start',**dict(intent,workload='probe-success'))['error']['code']=='conflict'
        assert self.request('start',**dict(intent,sequence=intent['sequence']+1))['error']['code']=='busy'
        self.request('cancel',incarnation=intent['incarnation'],sequence=intent['sequence'])
        assert self.terminal(intent)['outcome']=='cancelled'
        # The instance owner lock and separate endpoint claim both protect an existing listener.
        inode=self.socket.stat().st_ino
        duplicate=subprocess.run([str(self.launcher),'--config',str(self.root/'config.json')],capture_output=True,timeout=5)
        assert duplicate.returncode!=0 and self.socket.stat().st_ino==inode
        other=self.root.parent/uuid.uuid4().hex;other.mkdir(mode=0o700)
        config=dict(self.config,instance=other.name,root=str(other));path=other/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
        duplicate=subprocess.run([str(self.launcher),'--config',str(path)],capture_output=True,timeout=5)
        assert duplicate.returncode!=0 and self.socket.stat().st_ino==inode
        assert not Path('/sys/fs/cgroup','slipstreamprocessing'+other.name+'.slice').exists()
        claim=Path('/var/lib/slipstream-processing/instances',other.name+'.claim')
        assert json.loads(claim.read_text())['root']==str(other);claim.unlink()
        shutil.rmtree(other)
        assert self.request('reconcile')['result']['availability']=='available'

    def policy_and_epoch(self):
        # Drain the captured old attempt before a lowered operator policy becomes available.
        intent=self.intent('probe-hold');self.request('start',**intent)
        await_condition(lambda:self.receipt(intent)['state']=='running')
        self.stop(crash=True)
        self.config['memory_bytes']=64*1024*1024;self.save_config();self.start()
        previous=self.terminal(intent)
        assert previous['outcome']=='interrupted' and previous['limits']['memory_bytes']==128*1024*1024
        old=dict(intent,sequence=self.intent('probe-success')['sequence'],workload='probe-success')
        assert self.request('start',**old)['error']['code']=='incompatible-policy'
        self.run('probe-success','completed')
        self.stop();self.config['memory_bytes']=128*1024*1024;self.config['receipt_retention_seconds']=1;self.save_config();self.start()
        intent=self.intent('probe-success');self.request('start',**intent);self.terminal(intent)
        await_condition(lambda:self.request('inspect',incarnation=intent['incarnation'],sequence=intent['sequence']).get('error',{}).get('code')=='expired',3)
        self.stop();self.start()
        assert self.request('start',**intent)['error']['code']=='expired'
        self.stop();self.config['receipt_retention_seconds']=86400;self.save_config();self.start()
        # A new registry cannot adopt an old retained slice by mutable name alone.
        intent=self.intent('probe-success');self.arm(intent,'after-slice');self.request('start',**intent);self.marker();self.stop(crash=True);self.disarm()
        original=(self.root/'registry.json').read_bytes();(self.root/'registry.json').unlink()
        rejected=subprocess.run([str(self.launcher),'--config',str(self.root/'config.json')],capture_output=True,timeout=5)
        assert rejected.returncode!=0 and not (self.root/'registry.json').exists()
        group=Path('/sys/fs/cgroup',self.parent,json.loads(original)['records'][str(intent['sequence'])]['receipt']['runtime']['attempt_unit'])
        assert group.exists()
        self.stop();(self.root/'registry.json').write_bytes(original);(self.root/'registry.json').chmod(0o600);self.start();assert self.terminal(intent)['outcome']=='interrupted'

    def permitted_peer(self):
        self.stop();self.config['peer_uid']=1000;self.save_config();self.root.chmod(0o711)
        self.start();self.run('probe-success','completed');self.run('probe-native-oom','oom');self.run('probe-success','completed')
        assert self.socket.stat().st_uid==0
        acl=os.getxattr(self.socket,'system.posix_acl_access')
        (self.output/'permitted-peer.json').write_text(json.dumps(dict(observations=self.permitted_peer_observations,socket_acl_hex=acl.hex()),indent=2))

    def foreign_identity(self):
        receipt=self.run('probe-success','completed');runtime=receipt['runtime']
        self.stop()
        # Recreating an old slice's mutable name never restores its old ownership.
        command('systemctl','set-property','--runtime',runtime['attempt_unit'],'MemoryMax=134217728','MemorySwapMax=0','TasksMax=32','CPUQuota=100%')
        command('systemctl','start',runtime['attempt_unit'])
        self.log=(self.root/'launcher.log').open('ab')
        self.process=subprocess.Popen([str(self.launcher),'--config',str(self.root/'config.json')],stdout=self.log,stderr=subprocess.STDOUT)
        def blocked():
            try:return self.request('reconcile')['result']['availability']=='blocked'
            except (FileNotFoundError,ConnectionRefusedError):return False
        await_condition(blocked)
        group=Path('/sys/fs/cgroup',self.parent,runtime['attempt_unit']);assert group.exists()
        self.stop();command('systemctl','stop',runtime['attempt_unit']);command('systemctl','revert',runtime['attempt_unit'])
        # A foreign replacement keeps the label/name but has a different immutable ID.
        workspace=self.root/'attempts'/runtime['launch_id'];workspace.mkdir(mode=0o700)
        for name in ['control','work']:(workspace/name).mkdir(mode=0o700)
        arguments=['docker','create','--name','slipstream-processing-'+runtime['launch_id'],
            '--label','slipstream.processing.instance='+self.instance,
            '--label','slipstream.processing.launch='+runtime['launch_id'],
            '--label','slipstream.processing.incarnation='+receipt['incarnation'],
            '--cgroup-parent',runtime['attempt_unit'],'--cgroupns','private','--user','1000:1000',
            '--network','none','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges:true','--log-driver','none',
            '--memory',str(receipt['limits']['memory_bytes']),'--memory-swap',str(receipt['limits']['memory_bytes']),
            '--pids-limit','32','--cpus','1','--mount',f'type=bind,source={workspace}/control,target=/control,readonly']
        for target in ['/work','/tmp','/dev/shm']:arguments += ['--mount',f'type=bind,source={workspace}/work,target={target}']
        arguments += [self.arguments.worker_image,'probe-success',runtime['launch_id'],str(receipt['deadline_unix_ms'])]
        cid=command(*arguments)
        assert cid!=runtime['container_id']
        self.log=(self.root/'launcher.log').open('ab')
        self.process=subprocess.Popen([str(self.launcher),'--config',str(self.root/'config.json')],stdout=self.log,stderr=subprocess.STDOUT)
        await_condition(blocked)
        assert command('docker','inspect','--format','{{.Id}}',cid)==cid
        self.stop();command('docker','rm',cid);shutil.rmtree(workspace);self.start()

    def aggregate_claims(self):
        # Same instance, distinct root/socket: the global claim excludes the second owner.
        other=self.root.parent/uuid.uuid4().hex;other.mkdir(mode=0o700)
        config=dict(self.config,root=str(other),socket=str(other/'control.sock'))
        path=other/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
        parent=Path('/sys/fs/cgroup',self.parent)
        before=dict(inode=parent.stat().st_ino,maximum=(parent/'memory.max').read_text(),invocation=command('systemctl','show',self.parent,'--property=InvocationID','--value'))
        rejected=subprocess.run([str(self.launcher),'--config',str(path)],capture_output=True,timeout=5)
        assert rejected.returncode!=0 and not (other/'registry.json').exists()
        # The persisted binding rejects a different root after the first process exits too.
        self.stop()
        rejected=subprocess.run([str(self.launcher),'--config',str(path)],capture_output=True,timeout=5)
        assert rejected.returncode!=0 and not (other/'registry.json').exists()
        after=dict(inode=parent.stat().st_ino,maximum=(parent/'memory.max').read_text(),invocation=command('systemctl','show',self.parent,'--property=InvocationID','--value'))
        assert before==after;shutil.rmtree(other);self.start()
        # Replacing an idle aggregate while the launcher is alive cannot authorize a new child.
        command('systemctl','stop',self.parent);command('systemctl','start',self.parent)
        assert parent.stat().st_ino!=before['inode']
        intent=self.intent('probe-success')
        assert self.request('start',**intent)['error']['code']=='unavailable'
        self.stop()
        self.log=(self.root/'launcher.log').open('ab')
        self.process=subprocess.Popen([str(self.launcher),'--config',str(self.root/'config.json')],stdout=self.log,stderr=subprocess.STDOUT)
        def blocked():
            try:return self.request('reconcile')['result']['availability']=='blocked'
            except (FileNotFoundError,ConnectionRefusedError):return False
        await_condition(blocked)
        assert (parent/'memory.max').read_text()==before['maximum']
        (self.output/'aggregate-replacement.json').write_text(json.dumps(dict(before=before,recreated_inode=parent.stat().st_ino),indent=2))
        # This is deliberately the last check: the old binding stays quarantined.

    def stopped_recovery_crash(self):
        intent=self.intent('probe-hold');self.request('start',**intent)
        await_condition(lambda:self.receipt(intent)['state']=='running')
        accepted=self.receipt(intent);self.stop(crash=True)
        self.arm(intent,'after-exit')
        self.log=(self.root/'launcher.log').open('ab')
        self.process=subprocess.Popen([str(self.launcher),'--config',str(self.root/'config.json')],stdout=self.log,stderr=subprocess.STDOUT)
        marker=self.marker()
        record=json.loads((self.root/'registry.json').read_text())['records'][str(intent['sequence'])]
        assert record['termination_reason']=='interrupted'
        self.stop(crash=True);self.disarm();self.start()
        receipt=self.terminal(intent)
        assert receipt['outcome']=='interrupted' and receipt['deadline_unix_ms']==accepted['deadline_unix_ms']
        (self.output/'stopped-recovery-crash.json').write_text(json.dumps(dict(marker=marker,before=record,after=receipt),indent=2))

    def orphan_deadline(self):
        intent=self.intent('probe-hold');self.request('start',**intent)
        await_condition(lambda:self.receipt(intent)['state']=='running')
        receipt=self.receipt(intent);runtime=receipt['runtime'];deadline=receipt['deadline_unix_ms']
        self.stop(crash=True)
        def exited():
            self.health()
            state=json.loads(command('docker','inspect','--format','{{json .State}}',runtime['container_id']))
            return state if not state['Running'] else None
        state=await_condition(exited,32)
        assert state['ExitCode']==76 and not state['OOMKilled']
        self.start();receipt=self.terminal(intent)
        assert receipt['outcome']=='deadline' and receipt['deadline_unix_ms']==deadline
        self.run('probe-success','completed')
        # Cancellation survives a crash before the paused bootstrap receives a token.
        intent=self.intent('probe-hold');self.arm(intent,'after-release-intent')
        self.request('start',**intent);self.marker()
        self.request('cancel',incarnation=intent['incarnation'],sequence=intent['sequence'])
        self.stop(crash=True);self.disarm();self.start()
        receipt=self.terminal(intent)
        assert receipt['outcome']=='cancelled' and receipt['cancellation_requested']

    def foreign_provisioning(self):
        for case in ['foreign-parent','torn-claim','held-claim']:
            instance=uuid.uuid4().hex;root=self.root.parent/instance;root.mkdir(mode=0o700)
            config=dict(self.config,instance=instance,root=str(root),socket=str(root/'control.sock'),peer_uid=0)
            path=root/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
            claim=Path('/var/lib/slipstream-processing/instances',instance+'.claim')
            parent='slipstreamprocessing'+instance+'.slice'
            if case=='foreign-parent':
                command('systemctl','set-property','--runtime',parent,'MemoryMax=67108864','MemorySwapMax=0','TasksMax=32','CPUQuota=100%');command('systemctl','start',parent)
                group=Path('/sys/fs/cgroup',parent);identity=(group.stat().st_ino,(group/'memory.max').read_text())
            elif case=='torn-claim':
                claim.write_bytes(b'{"version":1');claim.chmod(0o600)
            else:
                claim.write_text(json.dumps(dict(version=1,root=str(root))));claim.chmod(0o600)
                registry=dict(version=1,instance=instance,incarnation=uuid.uuid4().hex,watermark=0,parent_pending=False,parent_identity=None,active=None,records={})
                (root/'registry.json').write_text(json.dumps(registry));(root/'registry.json').chmod(0o600)
                held=claim.open('r+');fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
            process=subprocess.Popen([str(self.launcher),'--config',str(path)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            try:
                if case=='torn-claim':
                    assert process.wait(timeout=5)!=0 and claim.read_bytes()==b'{"version":1'
                    assert not (root/'registry.json').exists()
                elif case=='held-claim':
                    assert process.wait(timeout=5)!=0
                    assert not Path('/sys/fs/cgroup',parent).exists()
                    assert not command('systemctl','list-units','--all','--no-legend','--plain',parent)
                    assert json.loads((root/'registry.json').read_text())==registry
                    held.close()
                    other=root/'other';other.mkdir(mode=0o700)
                    conflicting=dict(config,root=str(other),socket=str(other/'control.sock'))
                    (other/'config.json').write_text(json.dumps(conflicting));(other/'config.json').chmod(0o600)
                    rejected=subprocess.run([str(self.launcher),'--config',str(other/'config.json')],capture_output=True,timeout=5)
                    assert rejected.returncode!=0 and not (other/'registry.json').exists()
                    assert not Path('/sys/fs/cgroup',parent).exists()
                else:
                    original_config=self.config;original_socket=self.socket;original_instance=self.instance
                    self.config=config;self.socket=Path(config['socket']);self.instance=instance
                    try:
                        def blocked():
                            try:return self.request('reconcile')['result']['availability']=='blocked'
                            except (FileNotFoundError,ConnectionRefusedError):return None
                        await_condition(blocked)
                        assert (group.stat().st_ino,(group/'memory.max').read_text())==identity
                    finally:self.config=original_config;self.socket=original_socket;self.instance=original_instance
            finally:
                if process.poll() is None:process.terminate();process.wait(timeout=5)
                if case=='foreign-parent':command('systemctl','stop',parent);command('systemctl','revert',parent)
                claim.unlink();shutil.rmtree(root)

    def boundaries(self):
        # Operator-only pressure overrides, never client protocol parameters.
        for boundary in ['leaf','parent']:
            intent=self.intent('probe-descendant-oom');self.arm(intent,'after-release-intent')
            self.request('start',**intent);marker=self.marker();receipt=self.receipt(intent)
            runtime=receipt['runtime'];info=json.loads(command('docker','inspect',runtime['container_id']))[0]
            scope=Path('/sys/fs/cgroup',self.parent,runtime['attempt_unit'],'docker-'+runtime['container_id']+'.scope')
            leaf=scope/'workload'
            assert info['State']['Paused'] is True
            assert info['HostConfig']['LogConfig']['Type']=='none' and info['LogPath']==''
            assert info['Config']['User']=='1000:1000' and info['HostConfig']['ReadonlyRootfs']
            assert info['HostConfig']['NetworkMode']=='none' and not info['HostConfig']['Privileged']
            assert info['HostConfig']['CapDrop']==['ALL']
            assert (leaf/'memory.oom.group').read_text().strip()=='1'
            assert (leaf/'memory.swap.max').read_text().strip()=='0'
            assert (leaf/'cgroup.procs').read_text().strip()==str(info['State']['Pid'])
            assert Path('/proc',str(info['State']['Pid']),'cgroup').read_text().strip()=='0::'+str(leaf).removeprefix('/sys/fs/cgroup')
            assert self.parent not in Path('/proc',str(self.process.pid),'cgroup').read_text()
            if boundary=='leaf':(leaf/'memory.max').write_text(str(64*1024*1024))
            else:command('systemctl','set-property','--runtime',self.parent,'MemoryMax=67108864')
            snapshot={name:(leaf/name).read_text() for name in ['memory.max','memory.swap.max','memory.oom.group','cpu.max','pids.max','cgroup.procs']}
            self.release(marker);receipt=self.terminal(intent)
            assert receipt['outcome']=='oom'
            before=receipt['evidence']['parent_before'];after=receipt['evidence']['parent_after']
            assert (after['local_oom']>before['local_oom']) == (boundary=='parent')
            assert receipt['evidence']['attempt_after']['oom_group_kill']>0
            (self.output/(boundary+'-pressure.json')).write_text(json.dumps(dict(receipt=receipt,placement=snapshot,container=info),indent=2))
            command('systemctl','set-property','--runtime',self.parent,'MemoryMax=134217728')
            self.run('probe-success','completed')
        command('systemctl','set-property','--runtime',self.parent,'MemoryMax=67108864')
        intent=self.intent('probe-success')
        assert self.request('start',**intent)['error']['code']=='unavailable'
        command('systemctl','set-property','--runtime',self.parent,'MemoryMax=134217728')

    def storage_retention(self):
        for workload,kind in [('probe-storage-full','bytes'),('probe-inodes-full','inodes')]:
            intent=self.intent(workload);self.arm(intent,'after-evidence')
            self.request('start',**intent);marker=self.marker();receipt=self.receipt(intent)
            runtime=receipt['runtime'];work=self.root/'attempts'/runtime['launch_id']/'work'
            group=Path('/sys/fs/cgroup',self.parent,runtime['attempt_unit'])
            stats=os.statvfs(work)
            assert (work/'result').stat().st_size==4096
            assert stats.f_blocks*stats.f_frsize==16777216 and stats.f_files==64
            assert stats.f_bavail==0 if kind=='bytes' else stats.f_favail==0
            current=int((group/'memory.current').read_text())
            assert current>0 and (group/'cgroup.events').read_text().splitlines()[0]=='populated 0'
            newer=self.intent('probe-success')
            assert self.request('start',**newer)['error']['code']=='busy'
            (self.output/('retained-storage-'+kind+'.json')).write_text(json.dumps(dict(receipt=receipt,current=current,free_bytes=stats.f_bavail*stats.f_frsize,free_inodes=stats.f_favail),indent=2))
            self.stop(crash=True);self.disarm();self.start()
            assert self.terminal(intent)['outcome']=='storage-full'

    def cancellation(self):
        intent = self.intent('probe-hold')
        self.arm(intent,'after-release-intent')
        self.request('start',**intent)
        marker=self.marker()
        receipt=self.request('cancel',incarnation=intent['incarnation'],sequence=intent['sequence'])['result']['receipt']
        assert receipt['cancellation_requested']
        self.request('start',**intent)
        self.release(marker)
        assert self.terminal(intent)['outcome']=='cancelled'
        intent=self.intent('probe-hold');self.request('start',**intent)
        await_condition(lambda:self.receipt(intent)['state']=='running')
        self.request('cancel',incarnation=intent['incarnation'],sequence=intent['sequence'])
        receipt=self.terminal(intent)
        assert receipt['outcome']=='cancelled'
        late=self.request('cancel',incarnation=intent['incarnation'],sequence=intent['sequence'])['result']['receipt']
        assert late==receipt

    def verify(self):
        self.start();self.start_web()
        for workload,outcome in [('probe-success','completed'),('probe-native-oom','oom'),
            ('probe-descendant-oom','oom'),('probe-exit-137','engine-failed'),
            ('probe-storage-full','storage-full'),('probe-inodes-full','storage-full')]:
            receipt=self.run(workload,outcome)
            if outcome=='oom':
                assert receipt['evidence']['docker_oom_killed'] is True
                assert receipt['evidence']['attempt_after']['oom_kill']>receipt['evidence']['attempt_before']['oom_kill']
            if workload=='probe-exit-137':
                assert receipt['evidence']['docker_oom_killed'] is False
                assert receipt['evidence']['attempt_after']['oom_kill']==receipt['evidence']['attempt_before']['oom_kill']
            self.run('probe-success','completed')
        self.control_contract()
        self.boundaries()
        self.storage_retention()
        self.cancellation()
        for phase in ['after-intent','after-slice','after-create-response','after-container-bound',
                      'after-release-intent','after-exit','after-evidence','after-container-removal',
                      'after-storage-unmount','after-slice-stop']:
            self.crash(phase,'probe-descendant-oom' if phase=='after-exit' else 'probe-success')
        self.crash('after-exit','probe-storage-full')
        self.crash('after-exit','probe-exit-137')
        self.stopped_recovery_crash()
        self.pending_slice_stop()
        self.foreign_configuration_after_stop()
        self.orphan_deadline()
        self.foreign_provisioning()
        self.foreign_identity()
        self.policy_and_epoch()
        self.permitted_peer()
        self.web_request('/api/albums/'+self.album_id+'/rename',{'name':'Survived processing failures'})
        self.album_name='Survived processing failures';self.health()
        command('docker','restart',self.web)
        port=json.loads(command('docker','inspect',self.web))[0]['NetworkSettings']['Ports']['3000/tcp'][0]['HostPort']
        self.url='http://127.0.0.1:'+port
        def resumed():
            try:self.health();return True
            except (OSError,urllib.error.URLError):return False
        await_condition(resumed)
        (self.output/'web-observations.json').write_text(json.dumps(self.web_observations,indent=2))
        self.aggregate_claims()
        (self.output/'receipts.json').write_text(json.dumps(self.results,indent=2))
        (self.output/'identity.json').write_text(json.dumps(dict(instance=self.instance,
            launcher_sha256=self.launcher_sha256,verifier_sha256=self.verifier_sha256,
            worker_image=self.arguments.worker_image,web_image=self.arguments.web_image,
            host=command('uname','-r'),docker=json.loads(command('docker','info','--format','{{json .}}'))['CgroupDriver']),indent=2))
        print(json.dumps(dict(status='passed',attempts=len(self.results),evidence=str(self.output))))

    def cleanup(self):
        self.stop()
        # The verifier deletes only immutable IDs whose exact durable registry it created.
        registry=self.root/'registry.json'
        if registry.exists():
            shutil.copyfile(registry,self.output/'final-registry.json')
            records=json.loads(registry.read_text())['records'].values()
            for record in records:
                runtime=record['receipt']['runtime'];cid=runtime['container_id']
                if cid:
                    data=command('docker','inspect',cid,check=False)
                    if data and json.loads(data):
                        info=json.loads(data)[0]
                        assert info['Id']==cid and info['Config']['Labels']['slipstream.processing.launch']==record['launch_id']
                        command('docker','rm','--force',cid)
                work=self.root/'attempts'/record['launch_id']/'work'
                if os.path.ismount(work):command('umount',str(work))
                command('systemctl','stop',runtime['attempt_unit'],check=False)
                command('systemctl','revert',runtime['attempt_unit'],check=False)
        if self.web:command('docker','rm','--force',self.web)
        command('systemctl','stop',self.parent,check=False);command('systemctl','revert',self.parent,check=False)
        assert not command('docker','ps','-aq','--filter','label=slipstream.processing.instance='+self.instance)
        assert not Path('/sys/fs/cgroup',self.parent).exists()
        assert str(self.root) not in Path('/proc/self/mountinfo').read_text()
        shutil.copyfile(self.root/'launcher.log',self.output/'launcher.log')
        claim=Path('/var/lib/slipstream-processing/instances',self.instance+'.claim')
        if claim.exists():
            assert json.loads(claim.read_text())['root']==str(self.root);claim.unlink()
        shutil.rmtree(self.root)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--launcher',required=True)
    parser.add_argument('--worker-image',required=True)
    parser.add_argument('--web-image',required=True)
    parser.add_argument('--output',required=True)
    arguments=parser.parse_args()
    assert os.geteuid()==0,'run this isolated kernel verifier with sudo'
    for image in [arguments.worker_image,arguments.web_image]:
        assert image.startswith('sha256:') and len(image)==71,'use exact local immutable image IDs'
    verifier=Qualification(arguments)
    try:verifier.verify()
    finally:verifier.cleanup()


if __name__=='__main__':main()
