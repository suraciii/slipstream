"""Root-owned exclusive metadata save boundary (Linux/systemd, stdlib only)."""
from __future__ import annotations

import argparse
import errno
import hashlib
import fcntl
import grp
import json
import os
from pathlib import Path
import pwd
import re
import selectors
import signal
import socket
import stat
import secrets
import struct
import subprocess
import sys
import time

REQUEST_LIMIT = 2 * 1024 * 1024
RESPONSE_LIMIT = 32 * 1024 * 1024
COMMAND_TIMEOUT = 10
HELPER_TIMEOUT = 30
INSTANCE = re.compile(r"[a-z0-9][a-z0-9-]{0,47}\Z")
LOCAL_FILESYSTEMS = {"ext4", "xfs", "btrfs", "ext2", "ext3"}


class Refusal(Exception):
    pass


def command(*args: str) -> str:
    try:
        result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=COMMAND_TIMEOUT, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Refusal(f"command unavailable: {args[0]}: {error}") from error
    if result.returncode or len(result.stdout) > RESPONSE_LIMIT:
        raise Refusal(f"{args[0]} refused: {result.stderr[:4096].decode(errors='replace').strip()}")
    return result.stdout.decode("utf-8")


def secure_path(path: Path, *, directory: bool = False) -> os.stat_result:
    if not path.is_absolute():
        raise Refusal(f"path must be absolute: {path}")
    for part in (path, *path.parents):
        facts = part.lstat()
        if stat.S_ISLNK(facts.st_mode) or facts.st_uid != 0 or facts.st_mode & 0o022:
            raise Refusal(f"path is not root-owned and immutable to non-root: {part}")
    facts = path.lstat()
    if directory and not stat.S_ISDIR(facts.st_mode):
        raise Refusal(f"not a directory: {path}")
    if not directory and (not stat.S_ISREG(facts.st_mode) or facts.st_nlink != 1):
        raise Refusal(f"not a single-link regular file: {path}")
    return facts


def properties(unit: str) -> dict[str, str]:
    raw = command("/usr/bin/systemctl", "show", unit, "--no-pager",
                  "--property=LoadState,FragmentPath,DropInPaths,ControlGroup,KillMode,TriggeredBy,ActiveState,ExecStart,User,Group,Restart")
    return dict(line.split("=", 1) for line in raw.splitlines() if "=" in line)


def own_cgroup() -> Path:
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        if line.startswith("0::/"):
            return Path("/sys/fs/cgroup") / line[4:]
    raise Refusal("unified cgroup v2 is required")


def process_uids(pid: str) -> set[int]:
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("Uid:"):
            return {int(value) for value in line.split()[1:]}
    raise Refusal("process UID could not be inspected")


def process_cgroup(pid: str) -> str:
    for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            return line[3:]
    raise Refusal("process is outside unified cgroups")


class Supervisor:
    def __init__(self, instance: str):
        if not INSTANCE.fullmatch(instance):
            raise Refusal("invalid instance identifier")
        self.instance = instance
        self.config_path = Path(f"/etc/slipstream-metadata/{instance}/config.json")
        secure_path(self.config_path)
        self.config = json.loads(self.config_path.read_text())
        expected = {"backingRoot", "writerUid", "writerGid", "webUid", "webGid", "serviceUnit", "helper", "smbConfig"}
        if set(self.config) != expected:
            raise Refusal("configuration keys do not match the supervisor contract")
        for key in ("writerUid", "writerGid", "webUid", "webGid"):
            if type(self.config[key]) is not int or self.config[key] <= 0:
                raise Refusal(f"{key} must be a positive integer")
        if self.config["writerUid"] == self.config["webUid"] or self.config["writerGid"] == self.config["webGid"]:
            raise Refusal("writer and Web identities must be distinct")
        self.unit = f"slipstream-metadata-smb@{instance}.service"
        if self.config["serviceUnit"] != self.unit:
            raise Refusal("serviceUnit must be the instance's fixed SMB unit")
        self.root = Path(self.config["backingRoot"])
        self.helper = Path(self.config["helper"])
        self.smb = Path(self.config["smbConfig"])
        if self.smb != Path(f"/etc/slipstream-metadata/{instance}/smb.conf"):
            raise Refusal("smbConfig must be the fixed instance configuration path")
        self.runtime = Path(f"/run/slipstream-metadata/{instance}")
        secure_path(self.runtime, directory=True)
        self.marker = self.runtime / "fenced"
        self.publication_record = self.runtime / "publication.json"
        self.publication_pending = self.runtime / "publication.json.new"

    def identity_isolation(self, allowed: str) -> None:
        writer = self.config["writerUid"]
        accounts = [entry.pw_name for entry in pwd.getpwall() if entry.pw_uid == writer]
        if len(accounts) != 1 or pwd.getpwuid(writer).pw_shell not in ("/usr/sbin/nologin", "/sbin/nologin", "/bin/false"):
            raise Refusal("writer must be a unique non-login account")
        if pwd.getpwuid(writer).pw_gid != self.config["writerGid"]:
            raise Refusal("writer primary group does not match configuration")
        if any(entry.pw_gid == self.config["writerGid"] and entry.pw_uid != writer for entry in pwd.getpwall()):
            raise Refusal("another account shares the private writer group")
        writer_group = grp.getgrgid(self.config["writerGid"])
        if writer_group.gr_mem or writer_group.gr_name != accounts[0]:
            raise Refusal("writer requires a dedicated same-name group without supplementary members")
        for entry in Path("/proc").iterdir():
            if not entry.name.isdecimal():
                continue
            try:
                process_status = Path(f"/proc/{entry.name}/status").read_text().splitlines()
                state = next((line.split()[1] for line in process_status if line.startswith("State:")), "")
                if state in ("Z", "X"):
                    continue
                gids = set()
                for line in process_status:
                    if line.startswith(("Gid:", "Groups:")):
                        gids.update(int(value) for value in line.split()[1:])
                if self.config["writerGid"] in gids:
                    group = process_cgroup(entry.name)
                    if group != allowed and not group.startswith(allowed + "/"):
                        raise Refusal(f"writer group reused outside managed service (pid {entry.name})")
                if writer in process_uids(entry.name):
                    group = process_cgroup(entry.name)
                    if group != allowed and not group.startswith(allowed + "/"):
                        raise Refusal(f"writer identity reused outside managed service (pid {entry.name})")
            except (FileNotFoundError, ProcessLookupError):
                continue
        # A stopped unit must not later spawn another process with this identity.
        units = command("/usr/bin/systemctl", "list-unit-files", "--type=service", "--no-legend", "--no-pager")
        account = accounts[0]
        names = {line.split()[0] for line in units.splitlines() if line.split()}
        loaded = command("/usr/bin/systemctl", "list-units", "--all", "--type=service", "--plain", "--no-legend", "--no-pager")
        names.update(line.split()[0] for line in loaded.splitlines() if line.split())
        # Bare templates are not valid `show` targets. A neutral concrete
        # instance reveals fixed writer identities; actual loaded instances
        # above reveal identity settings expanded from instance specifiers.
        names = {name.replace("@.service", "@slipstream-identity-probe.service") for name in names}
        if not names:
            raise Refusal("systemd service identity inventory is empty")
        raw = command("/usr/bin/systemctl", "show", *sorted(names), "--property=Id,User,Group,SupplementaryGroups")
        for block in raw.strip().split("\n\n"):
            settings = dict(line.split("=", 1) for line in block.splitlines() if "=" in line)
            identities = {str(writer), account, str(self.config["writerGid"])}
            configured = {settings.get("User", ""), settings.get("Group", ""), *settings.get("SupplementaryGroups", "").split()}
            if identities & configured and settings.get("Id") != self.unit:
                raise Refusal(f"writer identity reused by {settings.get('Id')}")

    def validate_tree(self) -> None:
        if not self.root.is_absolute() or str(self.root) != os.path.normpath(self.root):
            raise Refusal("backingRoot must be a normalized absolute path")
        # The immediate private boundary prevents bypass by every unrelated user.
        boundary = self.root.parent
        facts = boundary.lstat()
        if (not stat.S_ISDIR(facts.st_mode) or facts.st_uid != 0 or
                facts.st_gid != self.config["writerGid"] or stat.S_IMODE(facts.st_mode) != 0o710):
            raise Refusal("backing parent must be root:writer 0710")
        for parent in boundary.parents:
            facts = secure_path(parent, directory=True)
            if os.listxattr(parent):
                raise Refusal(f"backing ancestor must not carry ACLs or extended attributes: {parent}")
            search = 0o010 if facts.st_gid == self.config["writerGid"] else 0o001
            if not facts.st_mode & search:
                raise Refusal(f"writer cannot traverse backing ancestor: {parent}")
        if os.listxattr(boundary):
            raise Refusal("private ancestor must not carry ACLs or extended attributes")
        device = self.root.lstat().st_dev
        mounts = json.loads(command("/usr/bin/findmnt", "--json", "--target", str(self.root), "--output", "FSTYPE,TARGET"))
        if mounts["filesystems"][0]["fstype"] not in LOCAL_FILESYSTEMS:
            raise Refusal("backing store must use an admitted local filesystem")
        if Path("/proc/sys/fs/protected_hardlinks").read_text().strip() != "1":
            raise Refusal("fs.protected_hardlinks=1 is required")
        for folder, directories, files in os.walk(self.root, followlinks=False):
            for path in [Path(folder), *(Path(folder) / name for name in directories + files)]:
                facts = path.lstat()
                if facts.st_dev != device:
                    raise Refusal(f"nested filesystem is not admitted: {path}")
                if stat.S_ISDIR(facts.st_mode):
                    if facts.st_uid != 0 or facts.st_gid != self.config["writerGid"] or stat.S_IMODE(facts.st_mode) != 0o3770:
                        raise Refusal(f"directory must be root:writer 3770: {path}")
                elif stat.S_ISREG(facts.st_mode) and facts.st_nlink == 1:
                    if path.suffix.lower() == ".xmp":
                        if facts.st_uid != self.config["writerUid"] or facts.st_gid != self.config["writerGid"] or facts.st_mode & 0o007:
                            raise Refusal(f"Sidecar must belong to writer and be private: {path}")
                    elif facts.st_uid != 0 or facts.st_gid != self.config["writerGid"] or stat.S_IMODE(facts.st_mode) != 0o440:
                        raise Refusal(f"Original must be root:writer 0440 before admission: {path}")
                else:
                    raise Refusal(f"symlink, hardlink, or special file is not admitted: {path}")
                # Samba stores ordinary DOS flags here; ACLs and all other
                # attributes remain forbidden and cannot broaden writer access.
                if set(os.listxattr(path)) - {"user.DOSATTRIB"}:
                    raise Refusal(f"extended attributes/ACLs require removal before admission: {path}")

    def validate(self) -> dict[str, str]:
        secure_path(self.config_path)
        if json.loads(self.config_path.read_text()) != self.config:
            raise Refusal("configuration changed; restart supervisor")
        helper_facts = secure_path(self.helper)
        if not helper_facts.st_mode & 0o111 or helper_facts.st_mode & 0o6000:
            raise Refusal("helper must be executable without set-id bits")
        secure_path(self.smb)
        secure_path(Path(__file__).absolute())
        secure_path(Path("/usr/local/lib/systemd/system/slipstream-metadata-supervisor@.service"))
        supervisor_unit = properties(f"slipstream-metadata-supervisor@{self.instance}.service")
        if (supervisor_unit.get("FragmentPath") != "/usr/local/lib/systemd/system/slipstream-metadata-supervisor@.service"
                or supervisor_unit.get("DropInPaths") or supervisor_unit.get("KillMode") != "control-group"):
            raise Refusal("supervisor must use supplied immutable unit with control-group recovery")
        service = properties(self.unit)
        fragment = Path(service.get("FragmentPath", ""))
        secure_path(fragment)
        if fragment != Path("/usr/local/lib/systemd/system/slipstream-metadata-smb@.service") or service.get("DropInPaths"):
            raise Refusal("managed SMB must use the immutable supplied unit without overrides")
        if service.get("KillMode") != "control-group" or service.get("TriggeredBy") or service.get("User") not in ("", "root"):
            raise Refusal("managed SMB must be root-started, KillMode=control-group, without socket activation")
        if service.get("LoadState") != "loaded" or service.get("ActiveState") != "active":
            raise Refusal("managed SMB must be loaded and active before Save")
        expected_exec = f"/usr/sbin/smbd --foreground --no-process-group --configfile={self.smb}"
        if expected_exec not in service.get("ExecStart", ""):
            raise Refusal("managed service ExecStart does not match fixed Samba configuration")
        if not Path("/sys/fs/cgroup/cgroup.controllers").is_file():
            raise Refusal("cgroup v2 is required")
        group = service.get("ControlGroup", "")
        if not group.startswith("/") or group == "/":
            raise Refusal("managed service has no dedicated cgroup")
        self.identity_isolation(group)
        self.validate_tree()
        effective = subprocess.run(["/usr/bin/testparm", "--suppress-prompt", str(self.smb)],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   timeout=COMMAND_TIMEOUT, check=False)
        if effective.returncode or b"Unknown parameter" in effective.stderr or b"Ignoring unknown" in effective.stderr:
            raise Refusal("Samba configuration contains unavailable/invalid parameters")
        settings = ((None, "server role", "standalone server"), (None, "smb2 leases", "No"),
                    (None, "smb3 directory leases", "No"), (None, "clustering", "No"),
                    ("library", "oplocks", "No"), ("library", "level2 oplocks", "No"),
                    ("library", "kernel oplocks", "No"), ("library", "durable handles", "No"),
                    ("library", "follow symlinks", "No"), ("library", "wide links", "No"),
                    ("library", "read only", "No"), ("library", "path", str(self.root)),
                    ("library", "force user", pwd.getpwuid(self.config["writerUid"]).pw_name),
                    ("library", "force group", pwd.getpwuid(self.config["writerUid"]).pw_name))
        for section, parameter, expected in settings:
            args = ["/usr/bin/testparm", "--suppress-prompt", f"--parameter-name={parameter}"]
            if section:
                args.append(f"--section-name={section}")
            args.append(str(self.smb))
            if command(*args).strip().lower() != expected.lower():
                raise Refusal(f"Samba effective {parameter} must be {expected}")
        shares = [line.strip()[1:-1] for line in effective.stdout.decode().splitlines()
                  if line.strip().startswith("[") and line.strip().endswith("]")]
        if set(shares) != {"global", "library"}:
            raise Refusal("Samba instance must export only the library share")
        return service

    def kill_descendants(self) -> None:
        # A process-group kill alone misses a helper child that called setsid.
        # The dedicated systemd cgroup is the authoritative lifetime boundary.
        group = own_cgroup()
        deadline = time.monotonic() + COMMAND_TIMEOUT
        while True:
            pids = set()
            for file in group.rglob("cgroup.procs"):
                pids.update(int(pid) for pid in file.read_text().split())
            pids.discard(os.getpid())
            if not pids:
                break
            for pid in pids:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if time.monotonic() >= deadline:
                raise Refusal("helper descendants did not exit; retaining managed-service mask")
            time.sleep(0.05)

    @staticmethod
    def _temporary_name(token: str) -> str:
        try:
            encoded = token.encode("ascii")
        except UnicodeEncodeError as error:
            raise Refusal("publication token is not ASCII") from error
        return f".slipstream-sidecar-{hashlib.sha256(encoded).hexdigest()}.tmp"

    def _open_confined_parent(self, parent: Path) -> tuple[int, os.stat_result]:
        """Walk from / with O_NOFOLLOW on every component; never reopen by path."""
        if (not parent.is_absolute() or str(parent) != os.path.normpath(str(parent))
                or not parent.is_relative_to(self.root)):
            raise Refusal("publication parent is outside the backing root")
        flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        descriptor = os.open("/", flags)
        current = Path("/")
        device = None
        try:
            for component in parent.parts[1:]:
                child = os.open(component, flags, dir_fd=descriptor)
                os.close(descriptor)
                descriptor = child
                current /= component
                facts = os.fstat(descriptor)
                if facts.st_uid != 0:
                    raise Refusal("publication ancestors must remain root-owned")
                if current == self.root:
                    device = facts.st_dev
                if device is not None:
                    if (facts.st_dev != device or facts.st_gid != self.config["writerGid"]
                            or stat.S_IMODE(facts.st_mode) != 0o3770):
                        raise Refusal("publication parent is not an admitted library directory")
                elif facts.st_mode & 0o022:
                    raise Refusal("publication ancestor is writable by non-root")
            return descriptor, os.fstat(descriptor)
        except OSError as error:
            os.close(descriptor)
            raise Refusal(f"publication parent cannot be safely reopened: {error}") from error
        except BaseException:
            os.close(descriptor)
            raise

    def require_fence(self) -> None:
        # A marker records intent, not exclusivity. Recheck the actual service
        # and its cgroup before authorizing any Library deletion.
        secure_path(self.marker)
        service = properties(self.unit)
        if (service.get("LoadState") != "masked"
                or service.get("ActiveState") not in ("inactive", "failed")):
            raise Refusal("publication recovery requires SMB still masked and stopped")
        group = service.get("ControlGroup", "")
        if group:
            if not group.startswith("/") or group == "/":
                raise Refusal("managed service cgroup is invalid")
            for procs in (Path("/sys/fs/cgroup") / group.lstrip("/")).rglob("cgroup.procs"):
                if procs.read_text().strip():
                    raise Refusal("managed service still has descendants")
        self.identity_isolation("/no-managed-writer-may-remain")

    @staticmethod
    def _sync_directory(path: Path) -> None:
        descriptor = os.open(str(path), os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    def _publication_parent(self, original_path: object) -> Path:
        if type(original_path) is not str or not original_path:
            raise Refusal("save request must name an Original Location")
        parts = original_path.split("/")
        if (original_path.startswith("/") or "\x00" in original_path
                or any(part in ("", ".", "..") for part in parts)):
            raise Refusal("Original Location is not a confined relative path")
        return self.root.joinpath(*parts[:-1])

    def _write_publication_record(self, original_path: object, token: str) -> None:
        parent = self._publication_parent(original_path)
        self.require_fence()
        temporary = self._temporary_name(token)
        descriptor, facts = self._open_confined_parent(parent)
        try:
            try:
                os.stat(temporary, dir_fd=descriptor, follow_symlinks=False)
            except FileNotFoundError:
                pass
            else:
                raise Refusal("the exact session staging path already exists")
        finally:
            os.close(descriptor)
        if self.publication_record.exists() or self.publication_pending.exists():
            raise Refusal("a prior publication record is still present")
        record = {
            "version": 1,
            "token": token,
            "temporary": temporary,
            "parent": str(parent),
            "parentDevice": facts.st_dev,
            "parentInode": facts.st_ino,
            "writerUid": self.config["writerUid"],
            "writerGid": self.config["writerGid"],
        }
        descriptor = os.open(
            str(self.publication_pending),
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
            0o600,
        )
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                json.dump(record, stream, separators=(",", ":"), sort_keys=True)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
        except Exception:
            if descriptor >= 0:
                os.close(descriptor)
            raise
        os.replace(self.publication_pending, self.publication_record)
        self._sync_directory(self.runtime)

    def _read_publication_record(self) -> dict | None:
        try:
            descriptor = os.open(
                str(self.publication_record),
                os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
            )
        except FileNotFoundError:
            return None
        try:
            facts = os.fstat(descriptor)
            if (not stat.S_ISREG(facts.st_mode) or facts.st_nlink != 1
                    or facts.st_uid != 0 or stat.S_IMODE(facts.st_mode) != 0o600):
                raise Refusal("publication record is not a root-owned private file")
            with os.fdopen(descriptor, "r", encoding="utf-8") as stream:
                descriptor = -1
                record = json.load(stream)
        except (OSError, ValueError) as error:
            raise Refusal(f"publication record is invalid: {error}") from error
        finally:
            if descriptor >= 0:
                os.close(descriptor)
        expected = {"version", "token", "temporary", "parent", "parentDevice", "parentInode", "writerUid", "writerGid"}
        if not isinstance(record, dict) or set(record) != expected:
            raise Refusal("publication record fields do not match the contract")
        if (record["version"] != 1 or type(record["token"]) is not str
                or type(record["temporary"]) is not str or type(record["parent"]) is not str
                or type(record["parentDevice"]) is not int or type(record["parentInode"]) is not int
                or type(record["writerUid"]) is not int or type(record["writerGid"]) is not int):
            raise Refusal("publication record values do not match the contract")
        if record["writerUid"] != self.config["writerUid"] or record["writerGid"] != self.config["writerGid"]:
            raise Refusal("publication record writer identity changed")
        if (not re.fullmatch(r"[0-9a-f]{64}", record["token"])
                or record["temporary"] != self._temporary_name(record["token"])):
            raise Refusal("publication record staging name does not match its token")
        return record

    def discard_pending_record(self) -> None:
        try:
            facts = self.publication_pending.lstat()
        except FileNotFoundError:
            return
        if (not stat.S_ISREG(facts.st_mode) or facts.st_nlink != 1
                or facts.st_uid != 0 or stat.S_IMODE(facts.st_mode) != 0o600):
            raise Refusal("pending publication record is not a root-owned private file")
        self.publication_pending.unlink()
        self._sync_directory(self.runtime)

    def discard_staged_publication(self) -> None:
        """Remove only a session's exact intended artifact under its intact fence."""
        record = self._read_publication_record()
        if record is None:
            return
        self.require_fence()
        parent = Path(record["parent"])
        descriptor, facts = self._open_confined_parent(parent)
        try:
            if (facts.st_dev != record["parentDevice"] or facts.st_ino != record["parentInode"]):
                raise Refusal("publication parent identity no longer matches the record; artifact retained")
            try:
                artifact = os.stat(record["temporary"], dir_fd=descriptor, follow_symlinks=False)
            except FileNotFoundError:
                artifact = None
            if artifact is not None:
                if (not stat.S_ISREG(artifact.st_mode) or artifact.st_nlink != 1
                        or artifact.st_uid != record["writerUid"]
                        or artifact.st_gid != record["writerGid"]):
                    raise Refusal("recorded staging artifact identity is invalid; artifact retained")
                os.unlink(record["temporary"], dir_fd=descriptor)
                os.fsync(descriptor)
                print(f"discarded attested staged publication under SMB fence: {parent / record['temporary']}", file=sys.stderr)
        finally:
            os.close(descriptor)
        self.publication_record.unlink()
        self._sync_directory(self.runtime)

    def recover(self) -> None:
        # No record means no Library cleanup, including marker-less startup.
        # Kill every helper descendant before inspecting any recorded artifact.
        if self.marker.exists():
            self.kill_descendants()
        elif self.publication_record.exists():
            raise Refusal("publication record exists without an active fence; artifact retained")
        self.discard_pending_record()
        self.discard_staged_publication()
        self.release()

    def release(self) -> None:
        if self.publication_record.exists() or self.publication_pending.exists():
            raise Refusal("publication ledger must be cleaned before external access resumes")
        if self.marker.exists():
            self.kill_descendants()
            print("publication recovery complete; helper cgroup empty; resuming SMB", file=sys.stderr)
            command("/usr/bin/systemctl", "unmask", "--runtime", self.unit)
            command("/usr/bin/systemctl", "start", self.unit)
            if properties(self.unit).get("ActiveState") != "active":
                raise Refusal("managed service did not resume; fence marker retained")
            self.marker.unlink()

    def run_helper(self, request: object) -> object:
        parent, lease = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        process = None
        try:
            process = subprocess.Popen([str(self.helper), "--root", str(self.root), "--lease-fd", str(lease.fileno())],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                       pass_fds=(lease.fileno(),), user=self.config["writerUid"],
                                       group=self.config["writerGid"], extra_groups=(), umask=0o007,
                                       start_new_session=True)
            lease.close()
            payload = json.dumps(request, separators=(",", ":"), allow_nan=False).encode() + b"\n"
            os.set_blocking(process.stdin.fileno(), False)
            pending = memoryview(payload)
            output = bytearray()
            deadline = time.monotonic() + HELPER_TIMEOUT
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                selector.register(process.stdin, selectors.EVENT_WRITE)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise Refusal("helper exceeded 30-second deadline")
                    for key, _ in selector.select(min(remaining, 0.1)):
                        if key.fileobj is process.stdin:
                            written = os.write(key.fd, pending[:65536])
                            pending = pending[written:]
                            if not pending:
                                selector.unregister(process.stdin)
                                process.stdin.close()
                            continue
                        block = os.read(key.fd, 65536)
                        if not block:
                            selector.unregister(key.fileobj)
                        else:
                            output.extend(block)
                            if len(output) > RESPONSE_LIMIT - 1:
                                raise Refusal("helper response exceeds 32 MiB")
            process.wait(timeout=max(0.01, deadline - time.monotonic()))
            if process.returncode:
                raise Refusal(f"helper exited with status {process.returncode}")
            result = json.loads(output, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
            if not isinstance(result, dict) or set(result) not in ({"ok"}, {"error"}):
                raise Refusal("helper response must contain exactly ok or error")
            if "error" in result:
                error = result["error"]
                if (not isinstance(error, dict) or set(error) != {"code", "message", "details"}
                        or not isinstance(error["code"], str) or not isinstance(error["message"], str)):
                    raise Refusal("helper error does not match code/message/details contract")
            return result
        finally:
            if process is not None:
                # Kill even after leader exit: no descendant may retain write access.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=COMMAND_TIMEOUT)
                self.kill_descendants()
            lease.close()
            parent.close()

    def save(self, request: object) -> object:
        session_started = False
        result = None
        try:
            service = self.validate()
            events = Path("/sys/fs/cgroup") / service["ControlGroup"].lstrip("/") / "cgroup.events"
            with events.open() as fence:
                self.marker.touch(mode=0o600, exist_ok=False)
                command("/usr/bin/systemctl", "mask", "--runtime", self.unit)
                if properties(self.unit).get("LoadState") != "masked":
                    raise Refusal("runtime mask did not prevent managed-service restart")
                command("/usr/bin/systemctl", "stop", self.unit)
                try:
                    fence.seek(0)
                    values = dict(line.split() for line in fence.read().splitlines())
                    if values.get("populated") != "0":
                        raise Refusal("managed service cgroup still has descendants")
                except OSError as error:
                    if error.errno != errno.ENODEV:
                        raise
                    # kernfs returns ENODEV for the retained events descriptor
                    # after systemd removes the stopped service's cgroup.
                    try:
                        events.parent.lstat()
                    except FileNotFoundError:
                        pass
                    else:
                        raise Refusal("retained cgroup descriptor failed but cgroup still exists") from error
                stopped = properties(self.unit)
                if stopped.get("LoadState") != "masked" or stopped.get("ActiveState") not in ("inactive", "failed"):
                    raise Refusal("managed service is not stopped with restart fenced")
                # Reinspect file ownership after all external writers have exited.
                self.identity_isolation("/no-managed-writer-may-remain")
                self.validate_tree()
                token = secrets.token_hex(32)
                # Check the exact token-derived path only after quiescence and
                # identity validation, then commit its root-owned ledger before
                # any helper process can create it.
                self._write_publication_record(request.get("originalPath") if isinstance(request, dict) else None, token)
                session_started = True
                request = {"leaseToken": token, "save": request}
                result = self.run_helper(request)
        except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
            result = failure("outcome_unknown" if session_started else "save_unavailable", str(error))
        finally:
            try:
                self.recover()
            except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
                result = failure("outcome_unknown" if session_started else "save_unavailable", f"external access recovery failed: {error}")
        return result


def failure(code: str, message: str) -> dict:
    return {"error": {"code": code, "message": message, "details": None}}


def read_request(connection: socket.socket) -> object:
    data = bytearray()
    while len(data) <= REQUEST_LIMIT:
        block = connection.recv(min(65536, REQUEST_LIMIT + 1 - len(data)))
        if not block:
            raise Refusal("request must end with one newline")
        data.extend(block)
        if b"\n" in data:
            if len(data) > REQUEST_LIMIT or not data.endswith(b"\n") or data.count(b"\n") != 1:
                raise Refusal("one newline-framed request of at most 2 MiB is required")
            return json.loads(data, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
    raise Refusal("request exceeds 2 MiB")


def serve(supervisor: Supervisor) -> None:
    socket_path = supervisor.runtime / "supervisor.sock"
    socket_path.unlink(missing_ok=True)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(socket_path))
        os.chown(socket_path, 0, supervisor.config["webGid"])
        os.chmod(socket_path, 0o660)
        # Parent is traversable by Web, but socket access still requires peer UID.
        os.chown(supervisor.runtime, 0, supervisor.config["webGid"])
        os.chmod(supervisor.runtime, 0o710)
        listener.listen(16)
        while True:
            connection, _ = listener.accept()
            with connection:
                connection.settimeout(5)
                _, uid, _ = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                if uid != supervisor.config["webUid"]:
                    continue
                try:
                    request = read_request(connection)
                    if not isinstance(request, dict):
                        raise Refusal("request must be an object")
                    with (supervisor.runtime / "session.lock").open("a") as lock:
                        fcntl.flock(lock, fcntl.LOCK_EX)
                        if request == {"operation": "status"}:
                            try:
                                supervisor.validate()
                                response = {"available": True}
                            except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
                                response = {"available": False, "reason": str(error)}
                        elif set(request) == {"operation", "request"} and request["operation"] == "save":
                            response = supervisor.save(request["request"])
                        else:
                            response = failure("invalid_input", "expected status or save operation")
                except (Refusal, OSError, ValueError) as error:
                    response = failure("invalid_input", str(error))
                payload = json.dumps(response, separators=(",", ":"), allow_nan=False).encode() + b"\n"
                if len(payload) > RESPONSE_LIMIT:
                    payload = json.dumps(failure("outcome_unknown", "response exceeds 32 MiB")).encode() + b"\n"
                try:
                    connection.sendall(payload)
                except OSError:
                    pass


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--instance", required=True)
    parser.add_argument("--recover", action="store_true")
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("the supervisor requires root")
    supervisor = Supervisor(args.instance)
    if args.recover:
        supervisor.recover()
    else:
        supervisor.recover()
        serve(supervisor)


if __name__ == "__main__":
    main()
