from contextlib import redirect_stderr, redirect_stdout
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
    if arguments[:3] == ("systemctl", "--system", "show") and arguments[3].startswith(
        "--property=PrivateTmp"
    ):
        properties = arguments[3].split("=", 1)[1].split(",")
        body = "".join(f"{name}=\n" for name in properties)
        return deployment.CommandResult(0, body + "ProtectProc=default\n", "")
    if arguments[:4] == ("systemctl", "--system", "show", "--property=MainPID"):
        return deployment.CommandResult(0, "4242\n", "")
    return deployment.CommandResult(1, "", "unknown command")


class RecordingStream:
    """Capture writes into one ordered event log to assert output ordering."""

    def __init__(self, events, tag):
        self.events = events
        self.tag = tag

    def write(self, text):
        self.events.append((self.tag, text))
        return len(text)

    def flush(self):
        return None


def capture_main(argv):
    """Run main() for real while recording every output write in order.

    Host commands are refused so the run stays hermetic; the web checks stop
    before any request when no token file or an invalid URL is supplied.
    """
    events = []

    def refused_command(arguments, **_kwargs):
        return subprocess.CompletedProcess(arguments, 1, stdout="", stderr="")

    with patch.object(deployment.subprocess, "run", refused_command):
        with redirect_stdout(RecordingStream(events, "stdout")), redirect_stderr(
            RecordingStream(events, "stderr")
        ):
            exit_code = deployment.main(argv)
    return exit_code, events


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
                "launcher-mount-namespace", "launcher-config", "launcher-service", "launcher-runtime",
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
                "launcher-mount-namespace", "launcher-config", "launcher-service", "launcher-runtime",
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

    def test_installed_unit_keeps_attempt_storage_visible_to_the_engine(self):
        with patch.object(deployment.os, "readlink", side_effect=lambda _: "mnt:[111]") as readlink:
            self.assertEqual(
                deployment.DeploymentSnapshot(
                    instance=INSTANCE, policy=POLICY, bundle=BUNDLE, command=fake_command
                )._mount_namespace_check(),
                deployment.Check("launcher-mount-namespace", True),
            )
        self.assertEqual(
            [call.args[0] for call in readlink.call_args_list],
            ["/proc/1/ns/mnt", "/proc/4242/ns/mnt"],
        )

    def test_installed_unit_with_private_mount_namespace_is_rejected(self):
        def namespaced_command(arguments):
            if arguments[:3] == ("systemctl", "--system", "show") and arguments[3].startswith(
                "--property=PrivateTmp"
            ):
                return deployment.CommandResult(
                    0,
                    "PrivateTmp=yes\nProtectSystem=strict\nProtectKernelTunables=yes\n"
                    "ReadWritePaths=/run/slipstream-processing/x /var/lib/slipstream-processing\n"
                    "RestrictAddressFamilies=AF_UNIX\n",
                    "",
                )
            return fake_command(arguments)

        result = deployment.DeploymentSnapshot(
            instance=INSTANCE, policy=POLICY, bundle=BUNDLE, command=namespaced_command
        )._mount_namespace_check()
        self.assertFalse(result.ok)
        self.assertEqual(result.reason, "launcher-private-mount-namespace")
        self.assertEqual(
            result.detail,
            "PrivateTmp=yes, ProtectSystem=strict, ProtectKernelTunables=yes, "
            "ReadWritePaths=/run/slipstream-processing/x /var/lib/slipstream-processing",
        )

    def test_other_mount_namespacing_properties_are_rejected(self):
        for name in ("ProtectHome", "PrivateDevices", "ProtectProc", "ExecPaths", "NoExecPaths"):
            with self.subTest(name=name):
                def command(arguments):
                    if arguments[:3] == ("systemctl", "--system", "show") and arguments[3].startswith(
                        "--property=PrivateTmp"
                    ):
                        return deployment.CommandResult(0, f"{name}=yes\n", "")
                    return fake_command(arguments)

                result = deployment.DeploymentSnapshot(
                    instance=INSTANCE, policy=POLICY, bundle=BUNDLE, command=command
                )._mount_namespace_check()
                self.assertEqual(result.reason, "launcher-private-mount-namespace")
                self.assertEqual(result.detail, f"{name}=yes")

    def test_running_launcher_mount_namespace_must_match_pid_one(self):
        checker = deployment.DeploymentSnapshot(
            instance=INSTANCE, policy=POLICY, bundle=BUNDLE, command=fake_command
        )
        with patch.object(deployment.os, "readlink", side_effect=["mnt:[111]", "mnt:[222]"]):
            result = checker._mount_namespace_check()
        self.assertEqual(result.reason, "launcher-private-mount-namespace")

        with patch.object(deployment.os, "readlink", side_effect=OSError):
            result = checker._mount_namespace_check()
        self.assertEqual(result.reason, "launcher-process-unavailable")

        def stopped_command(arguments):
            if arguments[:4] == ("systemctl", "--system", "show", "--property=MainPID"):
                return deployment.CommandResult(0, "0\n", "")
            return fake_command(arguments)

        result = deployment.DeploymentSnapshot(
            instance=INSTANCE, policy=POLICY, bundle=BUNDLE, command=stopped_command
        )._mount_namespace_check()
        self.assertEqual(result.reason, "launcher-process-unavailable")

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

    def test_attempt_cgroup_rejects_unbounded_cpu_or_tasks_and_missing_io(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroup = root / "cgroup"
            attempt = cgroup / "slipstreamprocessing" / "attempt"
            attempt.mkdir(parents=True)
            (cgroup / "cgroup.controllers").write_text("cpu io memory pids\n")
            (attempt / "memory.max").write_text(str(4 * 1024**3) + "\n")
            (attempt / "memory.swap.max").write_text("0\n")
            (attempt / "cpu.max").write_text("400000 100000\n")
            (attempt / "pids.max").write_text("256\n")
            (attempt / "io.stat").write_text("")
            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                paths=deployment.Paths(cgroup_root=cgroup, attempt_cgroup=attempt),
                command=fake_command,
            )
            self.assertEqual(checker._cgroup_check(), deployment.Check("attempt-cgroup", True))

            (attempt / "cpu.max").write_text("max 100000\n")
            self.assertEqual(checker._cgroup_check().reason, "attempt-cpu-unlimited")

            (attempt / "cpu.max").write_text("400000 100000\n")
            (attempt / "pids.max").write_text("max\n")
            self.assertEqual(checker._cgroup_check().reason, "attempt-pids-unlimited")

            (attempt / "pids.max").write_text("256\n")
            (attempt / "io.stat").unlink()
            self.assertEqual(checker._cgroup_check().reason, "attempt-io-accounting-unavailable")

            (attempt / "io.stat").write_text("x" * (deployment.MAX_CGROUP_IO_BYTES + 1))
            self.assertEqual(checker._cgroup_check().reason, "attempt-io-accounting-unavailable")

            (attempt / "io.stat").write_bytes(b"\xff")
            self.assertEqual(checker._cgroup_check().reason, "attempt-io-accounting-unavailable")

    def test_web_capability_requires_the_ready_condition_with_profiles(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("synthetic-token\n")
            token_file.chmod(0o600)

            def profile_entry(profile_id):
                return {
                    "profileId": profile_id,
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": None,
                }

            def ready_capability():
                return {
                    "state": "ready",
                    "bundleId": BUNDLE,
                    "incarnation": "a" * 32,
                    "exposure": {"minimumEv": 0.0, "maximumEv": 1.0, "stepEv": 0.001},
                    "profiles": [
                        profile_entry("sony-ilce-7rm5-arw"),
                        profile_entry("sony-ilce-7cm2-arw"),
                    ],
                    "stages": {"develop": "ready", "film": "unavailable"},
                }

            def checks_for(overrides, origin="https://photos.example.com"):
                def urlopen(request, timeout):
                    path = request.full_url.split("/", 3)[-1]
                    if path == "healthz":
                        return FakeResponse({"status": "ok"})
                    if path == "api/status":
                        return FakeResponse({"state": "published"})
                    if path == "api/overview":
                        return FakeResponse({"albums": []})
                    capability = ready_capability()
                    capability.update(overrides)
                    return FakeResponse(capability)

                checker = deployment.DeploymentSnapshot(
                    instance=INSTANCE,
                    policy=POLICY,
                    bundle=BUNDLE,
                    web_url=origin,
                    web_token_file=token_file,
                    urlopen=urlopen,
                )
                return checker._web_checks()

            ready = checks_for({})
            self.assertTrue(all(check.ok for check in ready), ready)
            http_ready = checks_for({}, "http://photos.example.com")
            self.assertTrue(all(check.ok for check in http_ready), http_ready)

            # Each row overrides one part of the ready answer; the reason and
            # detail are the contract the verifier reports for it.
            for description, overrides, reason, detail in [
                (
                    "a blocked deployment reports the observed condition",
                    {
                        "state": "resource-unavailable",
                        "stages": {"develop": "unavailable"},
                    },
                    "web-capability-unavailable",
                    "resource-unavailable",
                ),
                (
                    "an empty profile list is refused outside source-unsupported",
                    {"profiles": []},
                    "web-capability-unavailable",
                    "profiles",
                ),
                (
                    "a profile outside the closed qualified set is refused",
                    {"profiles": [profile_entry("unapproved-camera")]},
                    "web-capability-response-invalid",
                    "profiles",
                ),
                (
                    "a different well-formed bundle fails readiness",
                    {"bundleId": "d" * 64},
                    "web-capability-unavailable",
                    "bundle-mismatch",
                ),
                (
                    "a subset of the approved source classes is refused",
                    {"profiles": [profile_entry("sony-ilce-7rm5-arw")]},
                    "web-capability-response-invalid",
                    "profiles",
                ),
                (
                    "a profile id of the wrong JSON type is a failed check",
                    {
                        "profiles": [
                            profile_entry(["sony-ilce-7rm5-arw"]),
                            profile_entry("sony-ilce-7cm2-arw"),
                        ]
                    },
                    "web-capability-response-invalid",
                    "profiles",
                ),
            ]:
                checks = checks_for(overrides)
                self.assertEqual(checks[0].reason, reason, description)
                self.assertEqual(checks[0].detail, detail, description)


    def test_web_rejects_non_http_transport_before_sending_bearer(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("synthetic-token\n")
            token_file.chmod(0o600)

            def unexpected_urlopen(*_args, **_kwargs):
                raise AssertionError("bearer must not be sent over an unsupported transport")

            checker = deployment.DeploymentSnapshot(
                instance=INSTANCE,
                policy=POLICY,
                bundle=BUNDLE,
                web_url="ftp://photos.example.com",
                web_token_file=token_file,
                urlopen=unexpected_urlopen,
            )
            checks = checker._web_checks()
            self.assertEqual(checks[0].reason, "web-url-must-use-http-or-https")

    def test_main_warns_once_for_http_in_any_letter_case_before_checks(self):
        warning = (
            "Warning: HTTP is unencrypted; photos and credentials may be observed in transit."
        )
        for origin in (
            "http://photos.example.com",
            "HTTP://photos.example.com",
            "hTtP://photos.example.com",
        ):
            with self.subTest(origin=origin):
                # Without a token file the web checks stop at
                # web-token-required, so no request leaves the process.
                exit_code, events = capture_main(
                    [
                        "--instance", INSTANCE,
                        "--policy", POLICY,
                        "--bundle", BUNDLE,
                        "--web-url", origin,
                    ]
                )
                self.assertEqual(exit_code, 1)
                self.assertEqual(
                    "".join(text for tag, text in events if tag == "stderr"),
                    warning + "\n",
                )
                warning_writes = [
                    index
                    for index, (tag, text) in enumerate(events)
                    if tag == "stderr" and text.strip()
                ]
                report_writes = [
                    index
                    for index, (tag, text) in enumerate(events)
                    if tag == "stdout" and text
                ]
                self.assertEqual(len(warning_writes), 1)
                self.assertTrue(report_writes)
                self.assertLess(
                    warning_writes[0],
                    report_writes[0],
                    "the plaintext warning must precede the check report",
                )

    def test_main_does_not_warn_for_https_in_any_letter_case(self):
        for origin in ("https://photos.example.com", "HTTPS://photos.example.com"):
            with self.subTest(origin=origin):
                exit_code, events = capture_main(
                    [
                        "--instance", INSTANCE,
                        "--policy", POLICY,
                        "--bundle", BUNDLE,
                        "--web-url", origin,
                    ]
                )
                self.assertEqual(exit_code, 1)
                self.assertEqual([text for tag, text in events if tag == "stderr"], [])
                self.assertTrue(any(tag == "stdout" and text for tag, text in events))

    def test_main_keeps_an_invalid_web_url_as_a_failed_check_without_warning(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("synthetic-token\n")
            token_file.chmod(0o600)
            exit_code, events = capture_main(
                [
                    "--instance", INSTANCE,
                    "--policy", POLICY,
                    "--bundle", BUNDLE,
                    "--web-url", "http://[::1",
                    "--web-token-file", str(token_file),
                ]
            )
            self.assertEqual(exit_code, 1)
            self.assertEqual([text for tag, text in events if tag == "stderr"], [])
            report = json.loads("".join(text for tag, text in events if tag == "stdout"))
            self.assertEqual(report["status"], "read-only-checks-failed")
            web = next(
                check for check in report["checks"] if check["name"] == "web-capability"
            )
            self.assertIs(web["ok"], False)
            self.assertEqual(web["reason"], "web-url-invalid")


if __name__ == "__main__":
    unittest.main()
