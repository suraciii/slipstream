import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import verify_deployment as deployment


INSTANCE = "0123456789abcdef0123456789abcdef"
POLICY = "b" * 64
BUNDLE = "c" * 64


class FakeResponse:
    def __init__(self, value):
        self.status = 200
        self._body = json.dumps(value).encode()

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self, _size=-1):
        return self._body


def fake_command(arguments):
    if arguments[:4] == ("systemctl", "--system", "show", "--property=Version"):
        return deployment.CommandResult(0, "259\n", "")
    if arguments[:3] == ("docker", "info", "--format"):
        return deployment.CommandResult(0, "2 systemd\n", "")
    if arguments[:4] == ("systemctl", "--system", "show", "--property=ActiveState"):
        return deployment.CommandResult(0, "active\n", "")
    if arguments[:4] == ("systemctl", "--system", "show", "--property=SubState"):
        return deployment.CommandResult(0, "running\n", "")
    return deployment.CommandResult(1, "", "unknown command")


class DeploymentVerifierTests(unittest.TestCase):
    def test_production_probe_uses_web_uid_and_discards_output(self):
        completed = subprocess.CompletedProcess([], 0)
        with patch.object(deployment.subprocess, "run", return_value=completed) as run:
            self.assertIsNone(
                deployment.run_production_probe(Path("/usr/local/libexec/launcher"), INSTANCE, POLICY, BUNDLE)
            )
        arguments = run.call_args.args[0]
        self.assertEqual(
            arguments,
            (
                "/usr/bin/setpriv", "--reuid=1000", "--regid=1000", "--clear-groups",
                "--no-new-privs",
                "/usr/local/libexec/launcher", "--check-production", INSTANCE, POLICY, BUNDLE,
            ),
        )
        self.assertEqual(run.call_args.kwargs["stdin"], subprocess.DEVNULL)
        self.assertEqual(run.call_args.kwargs["stdout"], subprocess.DEVNULL)
        self.assertEqual(run.call_args.kwargs["stderr"], subprocess.DEVNULL)
        self.assertEqual(run.call_args.kwargs["timeout"], 5)
        self.assertEqual(run.call_args.kwargs["env"], {"PATH": "/usr/bin:/bin", "LANG": "C"})

    def test_production_probe_refuses_and_times_out_without_claiming_readiness(self):
        with patch.object(
            deployment.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)
        ):
            self.assertEqual(
                deployment.run_production_probe(Path("/launcher"), INSTANCE, POLICY, BUNDLE),
                "launcher-production-refused",
            )
        with patch.object(
            deployment.subprocess, "run", side_effect=subprocess.TimeoutExpired([], 5)
        ):
            self.assertEqual(
                deployment.run_production_probe(Path("/launcher"), INSTANCE, POLICY, BUNDLE),
                "launcher-production-timeout",
            )

    def test_production_check_requires_static_authority_before_probe(self):
        calls = []

        def probe(*arguments):
            calls.append(arguments)
            return None

        checker = deployment.DeploymentSnapshot(
            instance=INSTANCE, policy=POLICY, bundle=BUNDLE, production_probe=probe
        )
        required = [
            deployment.Check(name, True) for name in (
                "deployment-identities", "host-topology", "launcher-installation", "launcher-unit",
                "launcher-config", "launcher-service", "launcher-runtime",
            )
        ]
        required[-1] = deployment.Check("launcher-runtime", False, "launcher-socket-missing")
        self.assertEqual(
            checker._production_check(required).reason, "launcher-prerequisites-unavailable"
        )
        self.assertEqual(calls, [])

    def test_production_check_accepts_only_exact_web_uid_probe(self):
        required = [
            deployment.Check(name, True) for name in (
                "deployment-identities", "host-topology", "launcher-installation", "launcher-unit",
                "launcher-config", "launcher-service", "launcher-runtime",
            )
        ]
        observed = []

        def probe(*arguments):
            observed.append(arguments)
            return None

        checker = deployment.DeploymentSnapshot(
            instance=INSTANCE, policy=POLICY, bundle=BUNDLE, production_probe=probe
        )
        self.assertEqual(
            checker._production_check(required),
            deployment.Check("launcher-production-admission", True),
        )
        self.assertEqual(observed, [(checker.paths.launcher, INSTANCE, POLICY, BUNDLE)])

        checker.production_probe = lambda *_: "launcher-production-refused"
        self.assertEqual(
            checker._production_check(required).reason, "launcher-production-refused"
        )

    def test_missing_launcher_is_distinct_and_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                paths=deployment.Paths(
                    launcher=root / "missing-launcher",
                    unit=root / "unit",
                    config_root=root / "etc",
                    runtime_root=root / "run",
                    cgroup_root=root / "cgroup",
                ),
                command=fake_command,
            )
            snapshot = checker.run()
            launcher = next(item for item in snapshot["checks"] if item["name"] == "launcher-installation")
            self.assertEqual(launcher["reason"], "launcher-installation-missing")
            self.assertEqual(snapshot["status"], "read-only-checks-failed")
            self.assertFalse(snapshot["production_ready"])

    def test_missing_socket_is_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "run" / INSTANCE
            runtime.mkdir(parents=True)
            runtime.chmod(0o711)
            (runtime / "launcher.sock-owner").write_text("claim")
            (runtime / "launcher.sock-owner").chmod(0o600)
            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                paths=deployment.Paths(runtime_root=root / "run"),
                command=fake_command,
            )
            real_lstat = Path.lstat

            def root_owned_lstat(path):
                value = real_lstat(path)
                if Path(path) in {runtime, runtime / "launcher.sock-owner"}:
                    fields = list(value)
                    fields[4] = 0
                    value = type(value)(fields)
                return value

            with patch.object(Path, "lstat", root_owned_lstat):
                result = checker._runtime_check()
            self.assertEqual(result.reason, "launcher-socket-missing")

    def test_unlimited_attempt_memory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroup = root / "cgroup"
            attempt = cgroup / "slipstreamprocessing" / "attempt"
            attempt.mkdir(parents=True)
            (cgroup / "cgroup.controllers").write_text("cpu io memory pids\n")
            (attempt / "memory.max").write_text("max\n")
            (attempt / "memory.swap.max").write_text("0\n")
            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                paths=deployment.Paths(cgroup_root=cgroup, attempt_cgroup=attempt),
                command=fake_command,
            )
            self.assertEqual(checker._cgroup_check().reason, "attempt-memory-unlimited")

    def test_positive_finite_attempt_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroup = root / "cgroup"
            attempt = cgroup / "slipstreamprocessing" / "attempt"
            attempt.mkdir(parents=True)
            (cgroup / "cgroup.controllers").write_text("cpu io memory pids\n")
            (attempt / "memory.max").write_text(str(4 * 1024**3) + "\n")
            (attempt / "memory.swap.max").write_text("0\n")
            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                paths=deployment.Paths(cgroup_root=cgroup, attempt_cgroup=attempt),
                command=fake_command,
            )
            self.assertEqual(checker._cgroup_check(), deployment.Check("attempt-cgroup", True))

    def test_web_capability_requires_all_available_states(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("synthetic-token\n")
            token_file.chmod(0o600)

            def urlopen(request, timeout):
                path = request.full_url.split("/", 3)[-1]
                if path == "healthz":
                    value = {"status": "ok"}
                elif path == "api/status":
                    value = {"state": "published"}
                elif path == "api/overview":
                    value = {"albums": []}
                else:
                    value = {"state": "unavailable", "reason": "source-unavailable"}
                return FakeResponse(value)

            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                web_url="https://photos.example.com",
                web_token_file=token_file,
                urlopen=urlopen,
            )
            checks = checker._web_checks()
            self.assertEqual(checks[0].reason, "web-capability-unavailable")
            self.assertEqual(checks[0].detail, "source-unavailable")

    def test_web_rejects_plain_http_before_sending_bearer(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("synthetic-token\n")
            token_file.chmod(0o600)

            def unexpected_urlopen(*_args, **_kwargs):
                raise AssertionError("bearer must not be sent over HTTP")

            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                web_url="http://photos.example.com",
                web_token_file=token_file,
                urlopen=unexpected_urlopen,
            )
            checks = checker._web_checks()
            self.assertEqual(checks[0].reason, "web-url-must-use-https")


if __name__ == "__main__":
    unittest.main()
