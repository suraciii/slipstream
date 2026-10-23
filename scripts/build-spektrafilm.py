#!/usr/bin/env python3
"""Build Film normally, or prepare its exact-input CI executable artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FILM = Path("third_party/spektrafilm-rs")
BINARY = FILM / "target/release/spektrafilm-f64"
CARGO_COMMAND = [
    "cargo", "build", "--manifest-path", str(FILM / "Cargo.toml"),
    "--locked", "--release", "--features", "precision-f64",
    "-p", "spektrafilm-cli", "--bin", "spektrafilm-f64",
]


def output(command: list[str], cwd: Path, *, env=None) -> str:
    return subprocess.check_output(command, cwd=cwd, env=env, text=True).strip()


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def regular(path: Path, *, executable=False) -> None:
    mode = path.lstat().st_mode
    if not stat.S_ISREG(mode) or (executable and not mode & 0o111):
        raise ValueError(f"expected a regular{' executable' if executable else ''} file: {path}")


def source_inputs(root: Path) -> dict:
    source = root / FILM
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=source,
    ).decode().split("\0")
    files = {}
    for name in sorted(set(names) - {""}):
        path = source / name
        if not path.exists() and not path.is_symlink():
            files[name] = {"missing": True}
            continue
        regular(path)
        files[name] = {"sha256": digest(path), "mode": stat.S_IMODE(path.stat().st_mode)}
    return {"commit": output(["git", "rev-parse", "HEAD"], source), "files": files}


def artifact_policy(root: Path) -> None:
    # Reuse supports the workflow's one canonical build, not arbitrary Cargo
    # output layouts, compiler wrappers, or host-specific code generation.
    forbidden = {
        "CC", "CXX", "AR", "RUSTC", "RUSTDOC", "RUSTFLAGS", "RUSTDOCFLAGS",
        "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTDOC", "CARGO_BUILD_RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS", "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    }
    prefixes = ("CC_", "CXX_", "AR_", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS",
                "HOST_", "TARGET_", "CARGO_TARGET_", "CARGO_PROFILE_RELEASE_")
    for name, value in os.environ.items():
        if value and (name in forbidden or name.startswith(prefixes)):
            raise ValueError(f"CI Film artifacts do not support {name}")
    config_paths = [parent / ".cargo" / name for parent in (root, *root.parents)
                    for name in ("config", "config.toml")]
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    config_paths.extend(cargo_home / name for name in ("config", "config.toml"))
    for path in config_paths:
        if path.exists() or path.is_symlink():
            raise ValueError(f"CI Film artifacts do not support Cargo configuration: {path}")


def build_inputs(root: Path) -> dict:
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("CI Film artifacts require Linux x86_64")
    artifact_policy(root)
    source = source_inputs(root)
    pin = output(["git", "rev-parse", f"HEAD:{FILM}"], root)
    if source["commit"] != pin:
        raise ValueError("Film checkout does not match the pinned submodule commit")
    image = {name: os.environ[name] for name in ("ImageOS", "ImageVersion")}
    policy = {}
    for path in (root / "scripts/build-spektrafilm.py", root / "scripts/prepare-ci-libraw.sh",
                 root / "rust-toolchain.toml"):
        regular(path)
        policy[str(path.relative_to(root))] = digest(path)
    environment = {
        name: value for name, value in sorted(os.environ.items())
        if name.startswith(("CARGO_", "RUST", "CC_", "CXX_", "AR_", "PKG_CONFIG", "LIBRAW", "OPENBLAS"))
        or name in ("CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS", "LD_LIBRARY_PATH")
    }
    tools = {}
    for name, arguments in (("rustc", ["-vV"]), ("cargo", ["-V"]),
                            ("cc", ["--version"]), ("c++", ["--version"]),
                            ("ar", ["--version"]), ("ld", ["--version"])):
        selected = output(["rustup", "which", name], root) if name in ("rustc", "cargo") else shutil.which(name)
        path = Path(selected or name).resolve(strict=True)
        tools[name] = {"sha256": digest(path), "version": output([name, *arguments], root)}
    packages = output(["dpkg-query", "-W", "-f=${binary:Package}=${Version}\n"], root)
    native = {}
    for package in ("libraw", "libraw_r", "OpenImageIO", "exiv2", "lensfun", "glib-2.0", "Imath"):
        native[package] = output(["pkg-config", "--modversion", "--cflags", "--libs", package], root)
    prefix = Path(output(["pkg-config", "--variable=prefix", "libraw"], root))
    seal = prefix / "checksums.sha256"
    regular(seal)
    # The preparation owner checks every installed file before these inputs are used.
    # Bind its seal as well as the native package/image identity to the executable.
    return {
        "source": source, "policy": policy, "command": CARGO_COMMAND,
        "environment": environment, "tools": tools, "image": image,
        "native": native, "librawSeal": digest(seal),
        "packages": hashlib.sha256("\n".join(sorted(packages.splitlines())).encode()).hexdigest(),
        "platform": platform.machine(),
    }


def runtime_libraries(binary: Path) -> dict[str, str]:
    linkage = output(["ldd", str(binary)], binary.parent)
    libraries = {}
    for line in linkage.splitlines():
        if "not found" in line:
            raise ValueError(f"unresolved Film runtime library: {line.strip()}")
        match = re.search(r"(?:=>\s+)?(/\S+)\s+\(0x[0-9a-fA-F]+\)", line)
        if match:
            path = Path(match.group(1)).resolve(strict=True)
            libraries[str(path)] = digest(path)
    if not libraries:
        raise ValueError("Film has no resolved runtime library inventory")
    return libraries


def describe(binary: Path, commit: str) -> None:
    environment = dict(os.environ)
    environment.pop("SPEKTRAFILM_FORK_COMMIT", None)
    document = json.loads(output([str(binary), "describe", "--format", "json"], binary.parent, env=environment))
    if document.get("implementation") != "spektrafilm-rs" or document.get("forkCommit") != commit:
        raise ValueError("Film runtime identity does not match the pinned build")


def run_cargo(root: Path, commit: str | None = None) -> None:
    environment = dict(os.environ)
    if commit is not None:
        environment["SPEKTRAFILM_FORK_COMMIT"] = commit
    subprocess.run(CARGO_COMMAND, cwd=root, env=environment, check=True)


def validate(artifact: Path, inputs: dict) -> None:
    if artifact.is_symlink() or not artifact.is_dir():
        raise ValueError("Film artifact must be an ordinary directory")
    manifest = artifact / "manifest.json"
    binary = artifact / "spektrafilm-f64"
    regular(manifest)
    regular(binary, executable=True)
    document = json.loads(manifest.read_text())
    if not isinstance(document, dict) or document.get("format") != 1 or document.get("inputs") != inputs:
        raise ValueError("Film artifact inputs do not match this build")
    if document.get("binarySha256") != digest(binary):
        raise ValueError("Film artifact executable checksum mismatch")
    if document.get("libraries") != runtime_libraries(binary):
        raise ValueError("Film artifact runtime libraries changed")
    describe(binary, inputs["source"]["commit"])


def prepare(root: Path, artifact: Path) -> None:
    inputs = build_inputs(root)
    if not artifact.is_absolute():
        raise ValueError("Film artifact directory must be absolute")
    if not artifact.exists() and not artifact.is_symlink():
        run_cargo(root, inputs["source"]["commit"])
        built = root / BINARY
        regular(built, executable=True)
        artifact.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".film-build-", dir=artifact.parent) as temporary:
            staged = Path(temporary) / "artifact"
            staged.mkdir()
            binary = staged / "spektrafilm-f64"
            shutil.copy2(built, binary)
            document = {
                "format": 1, "inputs": inputs, "binarySha256": digest(binary),
                "libraries": runtime_libraries(binary),
            }
            (staged / "manifest.json").write_text(json.dumps(document, sort_keys=True) + "\n")
            validate(staged, inputs)
            staged.rename(artifact)
        print("Built and verified Film executable artifact", flush=True)
    else:
        validate(artifact, inputs)
        print("Reusing verified Film executable artifact without Cargo compilation", flush=True)
    destination = root / BINARY
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Never publish a partially copied executable or follow an existing output symlink.
    descriptor, temporary = tempfile.mkstemp(prefix=".film-", dir=destination.parent)
    os.close(descriptor)
    try:
        shutil.copyfile(artifact / "spektrafilm-f64", temporary)
        os.chmod(temporary, 0o755)
        os.replace(temporary, destination)
    finally:
        Path(temporary).unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-key", action="store_true")
    arguments = parser.parse_args()
    if arguments.cache_key:
        inputs = json.dumps(build_inputs(ROOT), sort_keys=True, separators=(",", ":")).encode()
        print(hashlib.sha256(inputs).hexdigest())
    elif artifact := os.environ.get("SLIPSTREAM_CI_FILM_ARTIFACT"):
        prepare(ROOT, Path(artifact))
    else:
        run_cargo(ROOT)


if __name__ == "__main__":
    main()
