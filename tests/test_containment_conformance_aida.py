#!/usr/bin/env python3
"""Binary-dependent shared-boundary fixture, separate from the fast evidence tests.
trace:TASK-3131 | ai:codex
"""
import json
import os
from pathlib import Path
import tempfile
import unittest

from tests.test_containment_conformance import c


class AidaProductionBoundaryTests(unittest.TestCase):
    def test_production_fixture_refuses_managed_role_enter_before_fake_vendor_exec(self):
        built = Path(__file__).resolve().parents[1] / "target/debug/aida"
        aida = str(built) if built.is_file() else c.shutil.which("aida")
        self.assertIsNotNone(aida, "production aida binary required (set PATH to a built binary)")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "bin"
            binaries.mkdir()
            vendor = binaries / "codex"
            marker = root / "vendor-executed"
            vendor.write_text("#!/bin/sh\nif [ \"$1\" = --version ]; then echo fixture-codex; exit 0; fi\n"
                              + "touch " + c.shlex.quote(str(marker)) + "\nexit 97\n")
            vendor.chmod(0o755)
            env = dict(os.environ, PATH=os.pathsep.join([str(binaries), str(Path(aida).parent), os.environ["PATH"]]),
                       AIDA_AGENT_TYPE="codex", AIDA_AGENT_NAME="offline-conformance-fixture")
            output = root / "experiment"
            result = c.subprocess.run([c.sys.executable, str(Path(c.__file__).resolve()),
                                       "--vendor", "codex", "--boundary", "aida", "--output", str(output)],
                                      env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn("fixture-role-enter-refused", result.stderr)
            self.assertIn("AIDA-managed agent sessions cannot issue direct TTY grants", result.stderr)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["boundary"], "aida")
            self.assertEqual(report["vendor_version"], "fixture-codex")
            self.assertIsNone(report["containment_receipt_sha256"])
            self.assertFalse(report["production_launcher_credit"])
            self.assertFalse(report["whole_lane_pass"])
            self.assertEqual({r["status"] for r in report["checks"].values()}, {"unknown"})
            self.assertEqual([r["stage"] for r in report["setup"]][-2:], ["role-scaffold", "role-enter"])
            self.assertNotEqual(report["setup"][-1]["exit_code"], 0)
            self.assertFalse(marker.exists())
            repo = output / "repository"
            policy = c.command(["git", "-C", str(repo), "show",
                                report["trusted_policy_sha"] + ":.aida/config.toml"])
            self.assertIn('codex = "aida"', policy)
            self.assertTrue((repo / ".aida-store/objects").is_dir())
            self.assertTrue((output / "authority-home/.aida/roles/implementer.toml").is_file())
            self.assertEqual(report["aida_binary_sha256"], c.hashlib.sha256(Path(aida).resolve().read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
