"""Copy the installed ELF dependency closure into the independent bundle."""
import shutil
import subprocess
from pathlib import Path

ROOT = Path("/opt/darktable-edit")


def main():
    output = ROOT / "lib"
    output.mkdir(exist_ok=True)
    def is_elf(path):
        if not path.is_file():
            return False
        with path.open("rb") as stream:
            return stream.read(4) == b"\x7fELF"
    pending = [path for path in (ROOT / "native").rglob("*") if is_elf(path)]
    visited = set()
    while pending:
        binary = pending.pop()
        if binary in visited:
            continue
        visited.add(binary)
        result = subprocess.run(["ldd", str(binary)], capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError(f"cannot inspect native ELF dependencies: {binary}: {result.stderr}")
        for line in result.stdout.splitlines():
            if "not found" in line:
                raise RuntimeError(f"unresolved installed native dependency: {binary}: {line}")
            fields = line.strip().split()
            dependency = fields[2] if len(fields) >= 3 and fields[1] == "=>" else fields[0] if fields else ""
            if not dependency.startswith("/"):
                continue
            source = Path(dependency)
            destination = output / source.name
            if destination.exists():
                if source.read_bytes() != destination.read_bytes():
                    raise RuntimeError(f"runtime dependency basename collision: {source}")
                continue
            shutil.copyfile(source, destination, follow_symlinks=True)
            destination.chmod(0o755)
            pending.append(destination)
    # Native install symlinks are dereferenced before digest pinning.
    for path in (ROOT / "native").rglob("*"):
        if path.is_symlink():
            raise RuntimeError(f"native install copy did not dereference symlink: {path}")


if __name__ == "__main__":
    main()
