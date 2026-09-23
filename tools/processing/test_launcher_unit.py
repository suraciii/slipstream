"""Deployment contract for the privileged host-side processing launcher."""
from pathlib import Path
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
UNIT = REPOSITORY_ROOT / "systemd" / "slipstream-processing-launcher@.service"


class LauncherSystemdUnitContract(unittest.TestCase):
    def test_launcher_cgroup_access_is_compatible_with_attempt_enforcement(self):
        directives = {
            line.split("=", 1)[0]: line.split("=", 1)[1]
            for line in UNIT.read_text().splitlines()
            if "=" in line and not line.lstrip().startswith("#")
        }

        self.assertEqual(directives["ProtectControlGroups"], "no")
        self.assertEqual(directives["ProtectSystem"], "strict")
        self.assertEqual(directives["ProtectKernelTunables"], "yes")
        self.assertEqual(directives["ProtectKernelModules"], "yes")
        self.assertEqual(directives["ProtectKernelLogs"], "yes")


if __name__ == "__main__":
    unittest.main()
