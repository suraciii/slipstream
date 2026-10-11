"""Export the standalone tool, native tree and runtime from an immutable image."""
import hashlib
import io
import json
import re
import subprocess
import tarfile
from pathlib import Path


def package_tool(image, destination):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}|[^\s]+@sha256:[0-9a-f]{64}", image):
        raise ValueError("standalone packaging requires an immutable image ID or digest reference")
    destination = Path(destination)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    destination.mkdir()
    container = subprocess.check_output(["docker", "create", image], text=True).strip()
    try:
        subprocess.run(["docker", "cp", f"{container}:/opt/darktable-edit", str(destination / "bundle")], check=True)
        for name in ("darktable-edit", "darktable-edit-supervisor"):
            subprocess.run(["docker", "cp", f"{container}:/usr/local/bin/{name}", str(destination / name)], check=True)
        subprocess.run(["docker", "cp", f"{container}:/usr/share/doc/slipstream/darktable-LICENSE", str(destination / "darktable-LICENSE")], check=True)
        import shutil
        notices = destination / "runtime-notices"
        subprocess.run(["docker", "cp", f"{container}:/usr/share/doc", str(notices)], check=True)
    finally:
        subprocess.run(["docker", "rm", container], check=True, stdout=subprocess.DEVNULL)
    manifest = destination / "bundle" / "edit-manifest.json"
    identity = hashlib.sha256(manifest.read_bytes()).hexdigest()
    if (destination / "bundle" / "bundle").read_text().strip() != identity:
        raise ValueError("exported bundle manifest digest mismatch")
    document = json.loads(manifest.read_bytes())
    files = {path.relative_to(destination / "bundle").as_posix(): path for path in (destination / "bundle").rglob("*") if path.is_file() and path.relative_to(destination / "bundle").as_posix() not in {"edit-manifest.json", "bundle"}}
    if set(files) != set(document["files"]):
        raise ValueError("exported bundle file coverage differs from manifest")
    for name, path in files.items():
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != document["files"][name]:
            raise ValueError(f"exported bundle asset digest mismatch: {name}")
    for source in (Path(__file__).resolve().parents[3] / "LICENSE", Path(__file__).resolve().parents[3] / "THIRD-PARTY-NOTICES.md"):
        if not source.is_file():
            raise ValueError(f"standalone license notice missing: {source}")
        import shutil
        shutil.copyfile(source, destination / source.name)
    (destination / "image.json").write_text(json.dumps({"image": image, "bundle": identity}, sort_keys=True) + "\n")
    # The executable itself stays beside its supervisor; the wrapper chooses only the shipped bundle.
    launcher = destination / "run-darktable-edit"
    launcher.write_text('#!/bin/sh\nset -eu\nroot=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexport DARKTABLE_EDIT_BUNDLE="$root/bundle"\nexec "$root/darktable-edit" "$@"\n')
    launcher.chmod(0o755)
    archive = destination.parent / f"{destination.name}.tar.gz"
    with archive.open("xb") as stream:
        with tarfile.open(fileobj=stream, mode="w:gz") as output:
            output.add(destination, arcname=destination.name, recursive=True)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
    return archive
