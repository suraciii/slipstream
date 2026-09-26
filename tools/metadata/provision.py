"""Provision a new isolated instance; never modifies source data or host Samba."""
from __future__ import annotations

import argparse
import ipaddress
import json
import os
from pathlib import Path
import pwd
import shutil
import stat
import subprocess

from supervisor import INSTANCE, LOCAL_FILESYSTEMS, Refusal, command, secure_path


def open_source(source: Path) -> int:
    descriptor = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for component in source.parts[1:]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instance", required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--web-user", required=True)
    parser.add_argument("--listen-address", required=True)
    parser.add_argument("--smb-port", type=int, default=1445)
    args = parser.parse_args()
    if os.geteuid() != 0 or not INSTANCE.fullmatch(args.instance):
        parser.error("requires root and an instance of 1-48 lowercase letters, digits, or hyphens")
    ipaddress.ip_address(args.listen_address)
    if not 1024 <= args.smb_port <= 65535:
        parser.error("isolated SMB port must be between 1024 and 65535")
    web = pwd.getpwnam(args.web_user)
    if web.pw_uid == 0:
        parser.error("Web must run as non-root")
    secure_path(args.helper)
    if not args.helper.stat().st_mode & 0o111:
        parser.error("helper must be executable")
    source = args.source.absolute()
    for part in (source, *source.parents):
        if stat.S_ISLNK(part.lstat().st_mode):
            parser.error(f"source path must not contain a symlink: {part}")
    if not source.is_dir():
        parser.error("source must be a directory")
    root = Path("/var/lib/slipstream-metadata")
    config_root = Path("/etc/slipstream-metadata")
    for parent in (root, config_root):
        parent.mkdir(mode=0o711, exist_ok=True)
        secure_path(parent, directory=True)
    for parent in (root, *root.parents):
        facts = secure_path(parent, directory=True)
        if os.listxattr(parent) or not facts.st_mode & 0o001:
            parser.error(f"backing ancestor must be searchable by the writer without ACLs: {parent}")
    destination = root / args.instance
    config_dir = config_root / args.instance
    if destination.exists() or config_dir.exists():
        parser.error("instance destination already exists; refusing any overwrite")
    filesystem = json.loads(command("/usr/bin/findmnt", "--json", "--target", str(root), "--output", "FSTYPE"))
    if filesystem["filesystems"][0]["fstype"] not in LOCAL_FILESYSTEMS:
        parser.error("instance storage must be ext2/ext3/ext4/xfs/btrfs")
    if Path("/proc/sys/fs/protected_hardlinks").read_text().strip() != "1":
        parser.error("requires fs.protected_hardlinks=1")
    # Reject links before copy; copyfile intentionally does not copy source ACLs.
    for folder, directories, files in os.walk(source, followlinks=False):
        for name in directories + files:
            path = Path(folder) / name
            facts = path.lstat()
            if not (stat.S_ISDIR(facts.st_mode) or stat.S_ISREG(facts.st_mode)) or (stat.S_ISREG(facts.st_mode) and facts.st_nlink != 1):
                parser.error(f"source contains a symlink, hardlink, or special file: {path}")
    writer_name = "ssmeta-" + args.instance
    if len(writer_name) > 31:
        parser.error("provisioned instance must be at most 24 characters (account name limit)")
    try:
        pwd.getpwnam(writer_name)
    except KeyError:
        command("/usr/sbin/useradd", "--system", "--user-group", "--no-create-home", "--shell", "/usr/sbin/nologin", writer_name)
    else:
        parser.error("writer account already exists; refusing identity reuse")
    writer = pwd.getpwnam(writer_name)
    destination.mkdir(mode=0o710)
    os.chown(destination, 0, writer.pw_gid)
    os.chmod(destination, 0o710)
    backing = destination / "library"
    backing.mkdir(mode=0o3770)
    os.chown(backing, 0, writer.pw_gid)
    os.chmod(backing, 0o3770)
    source_fd = open_source(source)
    try:
        for folder, directories, files, directory_fd in os.fwalk(".", dir_fd=source_fd, follow_symlinks=False):
            target = backing / folder
            for name in directories:
                facts = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
                if not stat.S_ISDIR(facts.st_mode):
                    raise Refusal(f"source directory changed or is a symlink: {name}")
                path = target / name
                path.mkdir(mode=0o3770)
                os.chown(path, 0, writer.pw_gid)
                os.chmod(path, 0o3770)
            for name in files:
                path = target / name
                descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory_fd)
                with os.fdopen(descriptor, "rb") as original:
                    facts = os.fstat(original.fileno())
                    if not stat.S_ISREG(facts.st_mode) or facts.st_nlink != 1:
                        raise Refusal(f"source file changed or is not a single-link regular file: {name}")
                    with path.open("xb") as copied:
                        shutil.copyfileobj(original, copied)
                sidecar = path.suffix.lower() == ".xmp"
                os.chown(path, writer.pw_uid if sidecar else 0, writer.pw_gid)
                os.chmod(path, 0o660 if sidecar else 0o440)
    finally:
        os.close(source_fd)
    config_dir.mkdir(mode=0o700)
    state = destination / "samba"
    state.mkdir(mode=0o700)
    for name in ("private", "lock", "state", "cache", "pid"):
        (state / name).mkdir(mode=0o700)
    smb = config_dir / "smb.conf"
    smb.write_text(f"""[global]
server role = standalone server
security = user
map to guest = Never
interfaces = {args.listen_address}
bind interfaces only = yes
smb ports = {args.smb_port}
disable netbios = yes
load printers = no
printing = bsd
printcap name = /dev/null
smb2 leases = no
smb3 directory leases = no
clustering = no
private dir = {state / 'private'}
lock directory = {state / 'lock'}
state directory = {state / 'state'}
cache directory = {state / 'cache'}
pid directory = {state / 'pid'}
log file = {state / 'smb.log'}
[library]
path = {backing}
valid users = {writer_name}
force user = {writer_name}
force group = {writer_name}
read only = no
oplocks = no
level2 oplocks = no
kernel oplocks = no
durable handles = no
follow symlinks = no
wide links = no
create mask = 0660
force create mode = 0660
directory mask = 0770
force directory mode = 0770
""")
    os.chmod(smb, 0o600)
    config = {"backingRoot": str(backing), "writerUid": writer.pw_uid, "writerGid": writer.pw_gid,
              "webUid": web.pw_uid, "webGid": web.pw_gid,
              "serviceUnit": f"slipstream-metadata-smb@{args.instance}.service",
              "helper": str(args.helper), "smbConfig": str(smb)}
    (config_dir / "config.json").write_text(json.dumps(config, indent=2) + "\n")
    os.chmod(config_dir / "config.json", 0o600)
    repository = Path(__file__).resolve().parents[2]
    executable_dir = Path("/usr/local/libexec")
    executable_dir.mkdir(parents=True, exist_ok=True)
    unit_dir = Path("/usr/local/lib/systemd/system")
    unit_dir.mkdir(parents=True, exist_ok=True)
    for name in ("slipstream-metadata-supervisor@.service", "slipstream-metadata-smb@.service"):
        target = unit_dir / name
        origin = repository / "systemd" / name
        if target.exists() and target.read_bytes() != origin.read_bytes():
            raise Refusal(f"installed shared unit differs; refusing overwrite: {target}")
        shutil.copyfile(origin, target)
        os.chown(target, 0, 0)
        os.chmod(target, 0o644)
    target = executable_dir / "slipstream-metadata-supervisor.py"
    origin = Path(__file__).with_name("supervisor.py")
    if target.exists() and target.read_bytes() != origin.read_bytes():
        raise Refusal("installed supervisor differs; refusing overwrite")
    shutil.copyfile(origin, target)
    os.chown(target, 0, 0)
    os.chmod(target, 0o755)
    command("/usr/bin/systemctl", "daemon-reload")
    print(json.dumps({"config": str(config_dir / "config.json"), "backingRoot": str(backing),
                      "writerUser": writer_name, "socket": f"/run/slipstream-metadata/{args.instance}/supervisor.sock"}))
    print(f"Set an isolated Samba password: sudo smbpasswd -c {smb} -a {writer_name}")
    print(f"Start: sudo systemctl start slipstream-metadata-smb@{args.instance}.service slipstream-metadata-supervisor@{args.instance}.service")


if __name__ == "__main__":
    main()
