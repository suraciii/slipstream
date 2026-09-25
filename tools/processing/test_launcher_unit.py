"""Deployment contract for the privileged host-side processing launcher."""
from pathlib import Path
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
UNIT = REPOSITORY_ROOT / "systemd" / "slipstream-processing-launcher@.service"

# Each of these directives gives the service a private mount namespace, so a
# mount the launcher performs beneath its instance root stays invisible to the
# host. The engine container resolves its bind sources in the host namespace,
# so it would bind the empty placeholder directory instead of the mounted
# attempt storage, and the fixed UID-1000 worker would fail to write its
# bounded result. The launcher's storage contract in
# `design/processing-executor.md` (Restricted Launch Authority) requires those
# mounts to resolve, so the unit must not enable any of them.
MOUNT_NAMESPACE_DIRECTIVES = (
    "PrivateTmp",
    "PrivateDevices",
    "ProtectHome",
    "ProtectProc",
    "ExecPaths",
    "NoExecPaths",
    "PrivateMounts",
    "ProtectSystem",
    "ProtectKernelTunables",
    "ProtectKernelModules",
    "ProtectKernelLogs",
    "ReadWritePaths",
    "ReadOnlyPaths",
    "InaccessiblePaths",
    "BindPaths",
    "BindReadOnlyPaths",
    "TemporaryFileSystem",
)


def directives() -> dict[str, str]:
    return {
        line.split("=", 1)[0]: line.split("=", 1)[1]
        for line in UNIT.read_text().splitlines()
        if "=" in line and not line.lstrip().startswith("#")
    }


class LauncherSystemdUnitContract(unittest.TestCase):
    def test_launcher_cgroup_access_is_compatible_with_attempt_enforcement(self):
        unit = directives()

        self.assertEqual(unit["ProtectControlGroups"], "no")
        self.assertEqual(unit["User"], "root")

    def test_attempt_storage_mounts_stay_visible_to_the_engine_container(self):
        unit = directives()

        for name in MOUNT_NAMESPACE_DIRECTIVES:
            value = unit.get(name)
            self.assertIsNone(
                value,
                f"{name} gives the launcher a private mount namespace, so its "
                f"attempt storage would not resolve for the engine container",
            )

    def test_launcher_keeps_the_remaining_service_hardening(self):
        unit = directives()

        self.assertEqual(unit["NoNewPrivileges"], "yes")
        self.assertEqual(unit["RestrictAddressFamilies"], "AF_UNIX")
        self.assertEqual(unit["UMask"], "0077")
        self.assertEqual(unit["Restart"], "on-failure")


if __name__ == "__main__":
    unittest.main()
