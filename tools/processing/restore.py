#!/usr/bin/env python3
"""Validate and restore a complete backup into an absent isolated root."""
import argparse
import hashlib
import json
import os
import subprocess
import tarfile
from pathlib import Path, PurePosixPath


def restore(source, destination, previous_state, server):
    destination = Path(destination).absolute()
    previous_state = str(Path(previous_state).absolute())
    with tarfile.open(source, "r:") as archive:
        members = archive.getmembers()
        names = [member.name for member in members]
        if len(names) != len(set(names)):
            raise ValueError("backup has duplicate paths")
        manifest_member = archive.getmember("backup-roots.json")
        if not manifest_member.isfile() or manifest_member.size > 64 * 1024 * 1024:
            raise ValueError("backup manifest is not a bounded regular file")
        manifest = json.load(archive.extractfile(manifest_member))
        if set(manifest) != {"format", "roots", "files"} or manifest["format"] != 1:
            raise ValueError("unsupported backup manifest")
        roots = manifest["roots"]
        if not roots or any(not key.startswith("root-") or not key[5:].isdigit() for key in roots):
            raise ValueError("invalid backup root mapping")
        state_keys = [key for key, value in roots.items() if value == previous_state]
        if len(state_keys) != 1:
            raise ValueError("previous state root must match exactly one captured root")
        evidence = {}
        for item in manifest["files"]:
            if set(item) != {"relativePath", "sha256", "byteLength"} or item["relativePath"] in evidence:
                raise ValueError("invalid or duplicate backup file evidence")
            evidence[item["relativePath"]] = item
        observed = set()
        for member in members:
            if member.name == "backup-roots.json":
                continue
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] not in roots or not (member.isdir() or member.isfile()):
                raise ValueError(f"unsafe backup member: {member.name}")
            if member.isfile():
                wanted = evidence.get(member.name)
                if wanted is None or wanted["byteLength"] != member.size:
                    raise ValueError(f"unmanifested/mis-sized member: {member.name}")
                digest = hashlib.sha256()
                with archive.extractfile(member) as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(chunk)
                if digest.hexdigest() != wanted["sha256"]:
                    raise ValueError(f"backup digest mismatch: {member.name}")
                observed.add(member.name)
        if observed != set(evidence):
            raise ValueError("manifest coverage differs from complete backup bytes")
        if any(destination == Path(root) or Path(root) in destination.parents for root in roots.values()):
            raise ValueError("restore destination must be isolated from captured original roots")
        destination.mkdir(mode=0o700)
        for key in roots:
            (destination / key).mkdir(mode=0o700)
        # Destination is private/new; no existing path or symlink can be overwritten.
        for member in members:
            if member.name == "backup-roots.json":
                continue
            target = destination / member.name
            if member.isdir():
                target.mkdir(mode=member.mode & 0o777, parents=True, exist_ok=True)
            else:
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                with target.open("xb") as output, archive.extractfile(member) as input_stream:
                    for chunk in iter(lambda: input_stream.read(1024 * 1024), b""):
                        output.write(chunk)
                    output.flush()
                    os.fsync(output.fileno())
                target.chmod(member.mode & 0o777)
                if hashlib.sha256(target.read_bytes()).hexdigest() != evidence[member.name]["sha256"]:
                    raise ValueError(f"restored digest mismatch: {member.name}")
        restored = {key: {"previousRoot": value, "restoredRoot": str(destination / key)} for key, value in roots.items()}
        restored_manifest = destination / "restore-manifest.json"
        restored_manifest.write_text(json.dumps({"format": 1, "roots": restored, "files": manifest["files"]}, sort_keys=True) + "\n")
        subprocess.run([str(server), "__restore-rebase", "--state", str(destination / state_keys[0]), "--manifest", str(restored_manifest)], check=True)
        return {"stateDirectory": str(destination / state_keys[0]), "manifest": str(restored_manifest), "complete": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--file", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--previous-state", type=Path, required=True)
    parser.add_argument("--server", type=Path, default=Path("slipstream-server"))
    args = parser.parse_args()
    print(json.dumps(restore(args.file, args.directory, args.previous_state, args.server), sort_keys=True))


if __name__ == "__main__":
    main()
