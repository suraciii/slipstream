"""Synthetic-token checks for the private Web survivor fixture."""
import hashlib
import json
from pathlib import Path
import os
import sqlite3
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import call, patch

import verify


class WebAccessFixture(unittest.TestCase):
    def test_start_web_seeds_digest_and_uses_loopback_bearer_probes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case = verify.Qualification.__new__(verify.Qualification)
            case.root = root
            case.instance = 'fixture-instance'
            case.arguments = SimpleNamespace(web_image='sha256:' + 'a' * 64)
            case.parent = 'slipstreamprocessing-fixture-test.slice'
            case.process = SimpleNamespace(pid=os.getpid())
            case.web = None
            case.web_token = None
            case.web_observations = []
            case.album_name = None

            docker_calls = []
            inspect_count = 0

            def fake_command(*arguments, **_kwargs):
                nonlocal inspect_count
                docker_calls.append(arguments)
                if arguments[:2] == ('docker', 'create'):
                    return 'fixture-container-id'
                if arguments[:2] == ('docker', 'inspect'):
                    inspect_count += 1
                    data = {'NetworkSettings': {'Ports': {'3000/tcp': [{'HostPort': '43127'}]}},
                            'State': {'Pid': os.getpid()}}
                    return json.dumps([data])
                return ''

            requests = []
            album_created = False

            class Response:
                status = 200

                def __init__(self, value):
                    self.content = json.dumps(value).encode()

                def __enter__(self):
                    return self

                def __exit__(self, *_args):
                    return False

                def read(self, *_args):
                    return self.content

            def fake_urlopen(request, timeout):
                nonlocal album_created
                requests.append((request, timeout))
                path = request.full_url.removeprefix(case.url)
                if path == '/healthz':
                    value = {'status': 'ok'}
                elif path == '/api/albums':
                    album_created = True
                    value = {'id': 'qualification-album'}
                elif path == '/api/overview':
                    value = {'published': True, 'photoCount': 0,
                             'albums': ([{'id': 'qualification-album', 'name': 'Qualification album'}]
                                        if album_created else [])}
                else:
                    raise AssertionError('unexpected Web fixture path')
                return Response(value)

            with patch.object(verify, 'command', side_effect=fake_command), \
                    patch.object(verify.os, 'chown') as chown, \
                    patch.object(verify.urllib.request, 'urlopen', side_effect=fake_urlopen):
                case.start_web()

            self.assertEqual(case.url, 'http://127.0.0.1:43127')
            self.assertEqual(case.album_id, 'qualification-album')
            self.assertEqual(inspect_count, 2)
            self.assertEqual(chown.call_args_list, [
                call(root / 'empty-library', 1000, 1000),
                call(root / 'web-state', 1000, 1000),
                call(root / 'web-cache', 1000, 1000),
                call(root / 'web-state' / 'access.sqlite', 1000, 1000),
            ])
            self.assertTrue(requests)
            private_requests = [request for request, _timeout in requests
                                if request.full_url.startswith(case.url + '/api/')]
            public_requests = [request for request, _timeout in requests
                               if request.full_url.endswith('/healthz')]
            self.assertTrue(private_requests)
            self.assertTrue(public_requests)
            self.assertTrue(all(request.get_header('Authorization', '').startswith('Bearer ')
                                for request in private_requests))
            bearer_values = [request.get_header('Authorization').removeprefix('Bearer ')
                             for request in private_requests]
            self.assertTrue(all(hashlib.sha256(value.encode()).digest()
                                == hashlib.sha256(case.web_token.encode()).digest()
                                for value in bearer_values))
            self.assertTrue(all(not request.has_header('Origin') for request, _ in requests))
            self.assertTrue(all(not request.has_header('Authorization') for request in public_requests))
            self.assertTrue(all(request.full_url.startswith('http://127.0.0.1:')
                                for request, _ in requests))

            create_args = next(args for args in docker_calls if args[:2] == ('docker', 'create'))
            self.assertIn('SLIPSTREAM_PUBLIC_ORIGIN=' + verify.QUALIFICATION_PUBLIC_ORIGIN,
                          create_args)
            self.assertIn('127.0.0.1::3000', create_args)
            command_text = '\0'.join(create_args)
            self.assertFalse(case.web_token in command_text,
                             'synthetic token must not appear in container arguments')

            with sqlite3.connect(root / 'web-state' / 'access.sqlite') as database:
                stored = database.execute('SELECT records FROM access WHERE id=1').fetchone()[0]
            records = json.loads(stored)
            self.assertEqual(records['credential']['digest'],
                             list(hashlib.sha256(case.web_token.encode()).digest()))
            self.assertEqual(records['sessions'], [])
            self.assertFalse(case.web_token in stored,
                             'synthetic token must not be persisted in clear')
            self.assertEqual((root / 'web-state' / 'access.sqlite').stat().st_mode & 0o777, 0o600)
            self.assertTrue(all(case.web_token not in json.dumps(observation)
                                for observation in case.web_observations))


if __name__ == '__main__':
    unittest.main()
