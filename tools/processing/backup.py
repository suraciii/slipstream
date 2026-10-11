#!/usr/bin/env python3
"""Archive quiesced state while excluding coordinated filesystem users."""
import argparse
import hashlib
import fcntl
import json
import os
import stat
import tarfile
from pathlib import Path


def open_directory(path):
    path = Path(path).absolute()
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in path.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def archive_directory(archive, descriptor, prefix, evidence):
    for name in sorted(os.listdir(descriptor)):
        metadata = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
        member = f"{prefix}/{name}"
        info = tarfile.TarInfo(member)
        info.mode = stat.S_IMODE(metadata.st_mode)
        info.uid, info.gid, info.mtime = metadata.st_uid, metadata.st_gid, int(metadata.st_mtime)
        if stat.S_ISDIR(metadata.st_mode):
            child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
            try:
                info.type = tarfile.DIRTYPE
                archive.addfile(info)
                archive_directory(archive, child, member, evidence)
            finally:
                os.close(child)
        elif stat.S_ISREG(metadata.st_mode):
            child = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
            with os.fdopen(child, "rb") as source:
                actual = os.fstat(source.fileno())
                if (actual.st_dev, actual.st_ino) != (metadata.st_dev, metadata.st_ino):
                    raise RuntimeError(f"backup input changed: {member}")
                info.size = actual.st_size
                digest = hashlib.sha256()
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
                source.seek(0)
                evidence.append({"relativePath": member, "sha256": digest.hexdigest(), "byteLength": actual.st_size})
                archive.addfile(info, source)
        else:
            raise ValueError(f"backup requires regular files/directories; unsupported entry: {member}")


def backup(roots, destination):
    roots = [Path(root).absolute() for root in roots]
    destination = Path(destination).absolute()
    if any(destination == root or root in destination.parents for root in roots):
        raise ValueError("backup output must be outside every captured root")
    if len(set(roots)) != len(roots) or any(a in b.parents for a in roots for b in roots if a != b):
        raise ValueError("backup roots must be distinct and non-overlapping")
    descriptors = []
    output = None
    try:
        for root in sorted(roots):
            descriptor = open_directory(root)
            descriptors.append((root, descriptor))
            # Nonblocking: fail without producing an archive while a CLI holds an ancestor lease.
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        output = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(output, "wb") as stream:
            output = None
            with tarfile.open(fileobj=stream, mode="w") as archive:
                mapping = {f"root-{index}": str(root) for index, (root, _) in enumerate(descriptors)}
                import io
                evidence = []
                for index, (_, descriptor) in enumerate(descriptors):
                    archive_directory(archive, descriptor, f"root-{index}", evidence)
                data = (json.dumps({"format": 1, "roots": mapping, "files": evidence}, sort_keys=True) + "\n").encode()
                info = tarfile.TarInfo("backup-roots.json")
                info.mode, info.size = 0o600, len(data)
                archive.addfile(info, io.BytesIO(data))
            stream.flush()
            os.fsync(stream.fileno())
        parent = open_directory(destination.parent)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)
        return mapping
    finally:
        if output is not None:
            os.close(output)
        for _, descriptor in reversed(descriptors):
            os.close(descriptor)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--include", type=Path, action="append", default=[], help="Additional workspace or global catalog root outside state")
    parser.add_argument("--file", type=Path, required=True)
    parser.add_argument("--service-stopped", action="store_true", required=True, help="Operator attests all service processes using these roots are stopped")
    arguments = parser.parse_args()
    mapping = backup([arguments.state, *arguments.include], arguments.file)
    print(json.dumps({"file": str(arguments.file.absolute()), "roots": mapping, "complete": True}, sort_keys=True))


if __name__ == "__main__":
    main()
