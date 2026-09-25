"""Read-only checks for a supported Slipstream processing deployment.

This snapshot checker never starts, stops, or reconfigures a service.  It is
deliberately separate from the fixture qualification runners: a fixture
socket, a healthy Library, or a running systemd unit is not enough to claim
Photo processing readiness.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import platform
import stat
import subprocess
import urllib.error
import urllib.parse
import urllib.request


DEFAULT_LAUNCHER = Path("/usr/local/libexec/slipstream-processing-launcher")
DEFAULT_UNIT = Path("/etc/systemd/system/slipstream-processing-launcher@.service")
DEFAULT_CONFIG_ROOT = Path("/etc/slipstream-processing")
DEFAULT_RUNTIME_ROOT = Path("/run/slipstream-processing")
DEFAULT_CGROUP_ROOT = Path("/sys/fs/cgroup")
MAX_WEB_BYTES = 1024 * 1024
MAX_WEB_TOKEN_BYTES = 4096
MAX_CGROUP_IO_BYTES = 4096


@dataclass(frozen=True)
class Paths:
    launcher: Path = DEFAULT_LAUNCHER
    unit: Path = DEFAULT_UNIT
    config_root: Path = DEFAULT_CONFIG_ROOT
    runtime_root: Path = DEFAULT_RUNTIME_ROOT
    cgroup_root: Path = DEFAULT_CGROUP_ROOT
    attempt_cgroup: Path | None = None


@dataclass(frozen=True)
class Check:
    name: str
    ok: bool
    reason: str | None = None
    detail: str | None = None

    def as_dict(self) -> dict[str, object]:
        value: dict[str, object] = {"name": self.name, "ok": self.ok}
        if self.reason is not None:
            value["reason"] = self.reason
        if self.detail is not None:
            value["detail"] = self.detail
        return value


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str
    stderr: str


def run_command(arguments: tuple[str, ...], timeout: float = 5.0) -> CommandResult:
    try:
        result = subprocess.run(
            arguments,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return CommandResult(1, "", type(error).__name__)
    return CommandResult(result.returncode, result.stdout, result.stderr)


def run_production_probe(launcher: Path, instance: str, policy: str, bundle: str) -> str | None:
    """Ask the installed launcher for exact Photo readiness as the Web UID."""
    try:
        result = subprocess.run(
            (
                "/usr/bin/setpriv",
                "--reuid=1000",
                "--regid=1000",
                "--clear-groups",
                "--no-new-privs",
                str(launcher),
                "--check-production",
                instance,
                policy,
                bundle,
            ),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env={"PATH": "/usr/bin:/bin", "LANG": "C"},
            timeout=5,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return "launcher-production-timeout"
    except OSError:
        return "launcher-production-probe-unavailable"
    return None if result.returncode == 0 else "launcher-production-refused"


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def open_web(request, timeout: float = 5.0):
    """Open one endpoint without forwarding the bearer token to a redirect."""
    return urllib.request.build_opener(_NoRedirect).open(request, timeout=timeout)


class DeploymentSnapshot:
    def __init__(
        self,
        *,
        instance: str,
        policy: str,
        bundle: str,
        paths: Paths = Paths(),
        web_url: str | None = None,
        web_token_file: Path | None = None,
        command=run_command,
        production_probe=run_production_probe,
        urlopen=open_web,
    ):
        self.instance = instance
        self.policy = policy
        self.bundle = bundle
        self.paths = paths
        self.web_url = web_url
        self.web_token_file = web_token_file
        self.command = command
        self.production_probe = production_probe
        self.urlopen = urlopen

    def run(self) -> dict[str, object]:
        checks = [
            self._identity_check(),
            self._host_check(),
            self._launcher_check(),
            self._unit_check(),
            self._config_check(),
            self._service_check(),
            self._runtime_check(),
            self._cgroup_check(),
        ]
        checks.append(self._production_check(checks))
        checks.extend(self._web_checks())
        passed = all(check.ok for check in checks)
        return {
            "scope": "read-only-deployment-snapshot",
            "status": "read-only-checks-passed" if passed else "read-only-checks-failed",
            "production_ready": False,
            "checks": [check.as_dict() for check in checks],
        }

    def _identity_check(self) -> Check:
        if not _lower_hex(self.instance, 32):
            return Check("deployment-identities", False, "invalid-instance")
        if not _lower_hex(self.policy, 64):
            return Check("deployment-identities", False, "invalid-policy")
        if not _lower_hex(self.bundle, 64):
            return Check("deployment-identities", False, "invalid-bundle")
        return Check("deployment-identities", True)

    def _host_check(self) -> Check:
        if platform.system() != "Linux":
            return Check("host-platform", False, "host-not-linux")
        cgroup = self.paths.cgroup_root
        controllers = cgroup / "cgroup.controllers"
        if not controllers.is_file():
            return Check("cgroup-v2", False, "cgroup-v2-unavailable")
        try:
            available = set(controllers.read_text().split())
        except OSError:
            return Check("cgroup-v2", False, "cgroup-v2-unreadable")
        required = {"memory", "cpu", "pids", "io"}
        missing = sorted(required - available)
        if missing:
            return Check("cgroup-v2", False, "cgroup-controller-missing", ",".join(missing))
        systemd = self.command(("systemctl", "--system", "show", "--property=Version", "--value"))
        if systemd.returncode != 0 or not systemd.stdout.strip():
            return Check("host-topology", False, "systemd-unavailable")
        docker = self.command(("docker", "info", "--format", "{{.CgroupVersion}} {{.CgroupDriver}}"))
        if docker.returncode != 0:
            return Check("host-topology", False, "docker-unavailable")
        if docker.stdout.strip() != "2 systemd":
            return Check("host-topology", False, "docker-cgroup-topology", docker.stdout.strip()[:80])
        return Check("host-topology", True)

    def _launcher_check(self) -> Check:
        return _regular_root_file(
            "launcher-installation", self.paths.launcher, executable=True
        )

    def _unit_check(self) -> Check:
        return _regular_root_file("launcher-unit", self.paths.unit)

    def _config_check(self) -> Check:
        if not _lower_hex(self.instance, 32):
            return Check("launcher-config", False, "invalid-instance")
        path = self.paths.config_root / self.instance / "config.json"
        result = _regular_root_file("launcher-config", path, root_only=True)
        if not result.ok:
            return result
        try:
            if len(path.read_bytes()) > 16 * 1024:
                return Check("launcher-config", False, "config-too-large")
        except OSError:
            return Check("launcher-config", False, "config-unreadable")
        return Check("launcher-config", True)

    def _service_check(self) -> Check:
        if not _lower_hex(self.instance, 32):
            return Check("launcher-service", False, "invalid-instance")
        unit = f"slipstream-processing-launcher@{self.instance}.service"
        active = self.command(("systemctl", "--system", "show", "--property=ActiveState", "--value", unit))
        if active.returncode != 0:
            return Check("launcher-service", False, "launcher-service-unavailable")
        if active.stdout.strip() != "active":
            return Check("launcher-service", False, "launcher-service-inactive", active.stdout.strip()[:80])
        sub = self.command(("systemctl", "--system", "show", "--property=SubState", "--value", unit))
        if sub.returncode != 0 or sub.stdout.strip() != "running":
            return Check("launcher-service", False, "launcher-service-not-running", sub.stdout.strip()[:80])
        return Check("launcher-service", True)

    def _runtime_check(self) -> Check:
        if not _lower_hex(self.instance, 32):
            return Check("launcher-runtime", False, "invalid-instance")
        runtime = self.paths.runtime_root / self.instance
        try:
            metadata = runtime.lstat()
        except FileNotFoundError:
            return Check("launcher-runtime", False, "runtime-directory-missing")
        except OSError:
            return Check("launcher-runtime", False, "runtime-directory-unreadable")
        if not stat.S_ISDIR(metadata.st_mode):
            return Check("launcher-runtime", False, "runtime-directory-not-directory")
        if metadata.st_uid != 0 or metadata.st_mode & 0o022:
            return Check("launcher-runtime", False, "runtime-directory-ownership")
        if metadata.st_mode & 0o777 != 0o711:
            return Check("launcher-runtime", False, "runtime-directory-mode")
        socket_path = runtime / "launcher.sock"
        claim_path = runtime / "launcher.sock-owner"
        try:
            socket_stat = socket_path.lstat()
        except FileNotFoundError:
            return Check("launcher-runtime", False, "launcher-socket-missing")
        except OSError:
            return Check("launcher-runtime", False, "launcher-socket-unreadable")
        if not stat.S_ISSOCK(socket_stat.st_mode):
            return Check("launcher-runtime", False, "launcher-socket-not-socket")
        if socket_stat.st_uid != 0:
            return Check("launcher-runtime", False, "launcher-socket-ownership")
        try:
            claim_stat = claim_path.lstat()
        except FileNotFoundError:
            return Check("launcher-runtime", False, "launcher-owner-claim-missing")
        except OSError:
            return Check("launcher-runtime", False, "launcher-owner-claim-unreadable")
        if (
            not stat.S_ISREG(claim_stat.st_mode)
            or claim_stat.st_uid != 0
            or claim_stat.st_nlink != 1
            or claim_stat.st_mode & 0o777 != 0o600
            or claim_stat.st_size > 4096
        ):
            return Check("launcher-runtime", False, "launcher-owner-claim-invalid")
        try:
            entries = sorted(entry.name for entry in runtime.iterdir())
        except OSError:
            return Check("launcher-runtime", False, "runtime-directory-unreadable")
        if entries != ["launcher.sock", "launcher.sock-owner"]:
            return Check("launcher-runtime", False, "runtime-directory-contents")
        return Check("launcher-runtime", True)

    def _cgroup_check(self) -> Check:
        attempt = self.paths.attempt_cgroup
        if attempt is None:
            return Check("attempt-cgroup", False, "attempt-cgroup-required")
        root = self.paths.cgroup_root
        if not attempt.is_absolute():
            return Check("attempt-cgroup", False, "attempt-cgroup-not-absolute")
        try:
            resolved_root = root.resolve(strict=True)
            resolved_attempt = attempt.resolve(strict=True)
            resolved_attempt.relative_to(resolved_root)
        except (FileNotFoundError, OSError, ValueError):
            return Check("attempt-cgroup", False, "attempt-cgroup-missing")
        memory = resolved_attempt / "memory.max"
        swap = resolved_attempt / "memory.swap.max"
        cpu = resolved_attempt / "cpu.max"
        pids = resolved_attempt / "pids.max"
        io_stat = resolved_attempt / "io.stat"
        try:
            memory_value = memory.read_text().strip()
            swap_value = swap.read_text().strip()
        except OSError:
            return Check("attempt-cgroup", False, "attempt-cgroup-limits-unreadable")
        if memory_value == "max":
            return Check("attempt-cgroup", False, "attempt-memory-unlimited")
        try:
            if int(memory_value) <= 0:
                return Check("attempt-cgroup", False, "attempt-memory-invalid")
        except ValueError:
            return Check("attempt-cgroup", False, "attempt-memory-invalid")
        if swap_value != "0":
            return Check("attempt-cgroup", False, "attempt-swap-not-zero", swap_value[:80])
        try:
            cpu_value = cpu.read_text().strip()
            pids_value = pids.read_text().strip()
        except OSError:
            return Check("attempt-cgroup", False, "attempt-cgroup-limits-unreadable")
        # Reading the controller's accounting file proves that the attempt
        # subtree has I/O accounting enabled. The contents are interpreted by
        # the retained terminal receipt, not this static snapshot.
        try:
            with io_stat.open(encoding="ascii") as stream:
                if len(stream.read(MAX_CGROUP_IO_BYTES + 1)) > MAX_CGROUP_IO_BYTES:
                    return Check("attempt-cgroup", False, "attempt-io-accounting-unavailable")
        except (OSError, UnicodeDecodeError):
            return Check("attempt-cgroup", False, "attempt-io-accounting-unavailable")
        cpu_parts = cpu_value.split()
        if len(cpu_parts) != 2 or any(part == "max" for part in cpu_parts):
            return Check("attempt-cgroup", False, "attempt-cpu-unlimited")
        try:
            cpu_quota, cpu_period = (int(part) for part in cpu_parts)
        except ValueError:
            return Check("attempt-cgroup", False, "attempt-cpu-invalid")
        if cpu_quota <= 0 or cpu_period <= 0:
            return Check("attempt-cgroup", False, "attempt-cpu-invalid")
        if pids_value == "max":
            return Check("attempt-cgroup", False, "attempt-pids-unlimited")
        try:
            if int(pids_value) <= 0:
                return Check("attempt-cgroup", False, "attempt-pids-invalid")
        except ValueError:
            return Check("attempt-cgroup", False, "attempt-pids-invalid")
        return Check("attempt-cgroup", True)

    def _production_check(self, checks: list[Check]) -> Check:
        required = {
            "deployment-identities",
            "host-topology",
            "launcher-installation",
            "launcher-unit",
            "launcher-config",
            "launcher-service",
            "launcher-runtime",
        }
        observed = {check.name: check for check in checks}
        if any(name not in observed or not observed[name].ok for name in required):
            return Check("launcher-production-admission", False, "launcher-prerequisites-unavailable")
        reason = self.production_probe(
            self.paths.launcher, self.instance, self.policy, self.bundle
        )
        if reason is not None:
            return Check("launcher-production-admission", False, reason)
        return Check("launcher-production-admission", True)

    def _web_checks(self) -> list[Check]:
        if not self.web_url:
            return [Check("web-capability", False, "web-url-required")]
        if self.web_token_file is None:
            return [Check("web-capability", False, "web-token-required")]
        try:
            token_stat = self.web_token_file.lstat()
            if not stat.S_ISREG(token_stat.st_mode) or token_stat.st_mode & 0o022:
                return [Check("web-capability", False, "web-token-file-ownership")]
            with self.web_token_file.open(encoding="utf-8") as stream:
                raw_token = stream.read(MAX_WEB_TOKEN_BYTES + 1)
        except OSError:
            return [Check("web-capability", False, "web-token-unreadable")]
        if len(raw_token) > MAX_WEB_TOKEN_BYTES:
            return [Check("web-capability", False, "web-token-too-large")]
        token = raw_token.strip()
        if not token or any(char.isspace() for char in token):
            return [Check("web-capability", False, "web-token-invalid")]
        try:
            parsed = urllib.parse.urlsplit(self.web_url)
            has_authority = bool(parsed.netloc)
            has_credentials = parsed.username is not None or parsed.password is not None
        except ValueError:
            return [Check("web-capability", False, "web-url-invalid")]
        if parsed.scheme != "https":
            return [Check("web-capability", False, "web-url-must-use-https")]
        if (
            not has_authority
            or has_credentials
            or parsed.path not in {"", "/"}
            or parsed.query
            or parsed.fragment
        ):
            return [Check("web-capability", False, "web-url-invalid")]
        values: dict[str, object] = {}
        for path in ["/healthz", "/api/status", "/api/overview", "/api/processing/capability"]:
            try:
                values[path] = self._web_json(path, token)
            except WebCheckError as error:
                return [Check("web-capability", False, error.reason, error.detail)]
        health = values["/healthz"]
        if not isinstance(health, dict) or health.get("status") != "ok":
            return [Check("web-capability", False, "web-health-invalid")]
        for path in ["/api/status", "/api/overview"]:
            if not isinstance(values[path], dict):
                return [Check("web-capability", False, "web-library-response-invalid", path)]
        overview = values["/api/overview"]
        if not isinstance(overview.get("albums"), list):
            return [Check("web-capability", False, "web-album-response-invalid")]
        capability = values["/api/processing/capability"]
        if not isinstance(capability, dict):
            return [Check("web-capability", False, "web-capability-response-invalid")]
        # The merged Photo Development service surface closes the capability
        # contract: a qualified deployment reports `ready` with a proven
        # develop stage, the approved per-class profiles, and the launcher
        # identities it observed.
        if capability.get("state") != "ready":
            return [
                Check("web-capability", False, "web-capability-unavailable", str(capability.get("state")))
            ]
        stages = capability.get("stages")
        if not isinstance(stages, dict) or stages.get("develop") != "ready":
            return [Check("web-capability", False, "web-capability-unavailable", "develop-stage")]
        if stages.get("film") != "unavailable":
            return [Check("web-capability", False, "web-capability-response-invalid", "film-stage")]
        bundle_id = capability.get("bundleId")
        incarnation = capability.get("incarnation")
        if (
            not isinstance(bundle_id, str)
            or not _lower_hex(bundle_id, 64)
            or not isinstance(incarnation, str)
            or not _lower_hex(incarnation, 32)
        ):
            return [Check("web-capability", False, "web-capability-response-invalid", "identities")]
        if capability.get("exposure") != {
            "minimumEv": 0.0,
            "maximumEv": 1.0,
            "stepEv": 0.001,
        }:
            return [Check("web-capability", False, "web-capability-response-invalid", "exposure")]
        profiles = capability.get("profiles")
        if not isinstance(profiles, list) or not profiles:
            # The profile list stays empty only for `source-unsupported`,
            # which cannot be `ready`.
            return [Check("web-capability", False, "web-capability-unavailable", "profiles")]
        # The qualified profile set is closed: a ready deployment may only
        # advertise profiles the contract approves.
        qualified_profile_ids = {"sony-ilce-7rm5-arw", "sony-ilce-7cm2-arw"}
        for profile in profiles:
            if (
                not isinstance(profile, dict)
                or not isinstance(profile.get("profileId"), str)
                or profile.get("profileId") not in qualified_profile_ids
                or profile.get("whiteBalanceModes") != ["as-shot"]
                or profile.get("whiteBalanceRanges") is not None
            ):
                return [
                    Check("web-capability", False, "web-capability-response-invalid", "profiles")
                ]
        return [
            Check("web-health", True),
            Check("web-library", True),
            Check("web-albums", True),
            Check("web-capability", True),
        ]

    def _web_json(self, path: str, token: str) -> object:
        request = urllib.request.Request(
            urllib.parse.urljoin(self.web_url.rstrip("/") + "/", path.lstrip("/")),
            headers={"Accept": "application/json", "Authorization": f"Bearer {token}"},
            method="GET",
        )
        try:
            with self.urlopen(request, timeout=5) as response:
                if response.status != 200:
                    raise WebCheckError("web-http-status", str(response.status))
                body = response.read(MAX_WEB_BYTES + 1)
        except urllib.error.HTTPError as error:
            raise WebCheckError("web-http-status", str(error.code)) from error
        except (urllib.error.URLError, OSError, TimeoutError) as error:
            raise WebCheckError("web-unreachable", type(error).__name__) from error
        if len(body) > MAX_WEB_BYTES:
            raise WebCheckError("web-response-too-large")
        try:
            return json.loads(body)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise WebCheckError("web-response-invalid") from error


class WebCheckError(Exception):
    def __init__(self, reason: str, detail: str | None = None):
        super().__init__(reason)
        self.reason = reason
        self.detail = detail


def _lower_hex(value: str, length: int) -> bool:
    return len(value) == length and all(char in "0123456789abcdef" for char in value)


def _regular_root_file(name: str, path: Path, *, executable: bool = False, root_only: bool = False) -> Check:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return Check(name, False, f"{name}-missing")
    except OSError:
        return Check(name, False, f"{name}-unreadable")
    if not stat.S_ISREG(metadata.st_mode):
        return Check(name, False, f"{name}-not-regular")
    if metadata.st_uid != 0 or metadata.st_mode & 0o022:
        return Check(name, False, f"{name}-ownership")
    if root_only and metadata.st_mode & 0o077:
        return Check(name, False, f"{name}-mode")
    if executable and not metadata.st_mode & 0o111:
        return Check(name, False, f"{name}-not-executable")
    return Check(name, True)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instance", required=True)
    parser.add_argument("--policy", required=True)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--attempt-cgroup", type=Path)
    parser.add_argument("--web-url", default=os.environ.get("SLIPSTREAM_DEPLOYMENT_WEB_URL"))
    parser.add_argument("--web-token-file", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(argv)
    snapshot = DeploymentSnapshot(
        instance=arguments.instance,
        policy=arguments.policy,
        bundle=arguments.bundle,
        paths=Paths(attempt_cgroup=arguments.attempt_cgroup),
        web_url=arguments.web_url,
        web_token_file=arguments.web_token_file,
    ).run()
    rendered = json.dumps(snapshot, indent=2, sort_keys=True) + "\n"
    print(rendered, end="")
    return 0 if snapshot["status"] == "read-only-checks-passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
