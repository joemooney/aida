#!/usr/bin/env python3
"""Binary-dependent shared-boundary fixture, separate from the fast evidence tests.
trace:TASK-3131 | ai:codex
"""
import json
import os
from pathlib import Path
import pty
import tempfile
import tomllib
import unittest

from tests.test_containment_conformance import c


class AidaProductionBoundaryTests(unittest.TestCase):
    # trace:BUG-2460 trace:BUG-2462 trace:BUG-2465 | ai:codex
    def test_terminal_stdin_fixture_initializes_then_refuses_managed_role_enter(self):
        built = Path(__file__).resolve().parents[1] / "target/debug/aida"
        aida = str(built) if built.is_file() else c.shutil.which("aida")
        self.assertIsNotNone(aida, "production aida binary required (set PATH to a built binary)")
        # Production tmpfs hides /tmp and /run; exercise role refusal from
        # an admitted output location. trace:TASK-3194 | ai:codex
        fixture_home = Path.home() / ".aida"
        fixture_home.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=fixture_home) as directory:
            root = Path(directory)
            binaries = root / "bin"
            binaries.mkdir()
            vendor = binaries / "codex"
            marker = root / "vendor-executed"
            vendor.write_text("#!/bin/sh\nif [ \"$1\" = --version ]; then echo fixture-codex; exit 0; fi\n"
                              + "touch " + c.shlex.quote(str(marker)) + "\nexit 97\n")
            vendor.chmod(0o755)
            env = dict(os.environ, PATH=os.pathsep.join([str(binaries), str(Path(aida).parent), os.environ["PATH"]]),
                       AIDA_AGENT_TYPE="codex", AIDA_AGENT_NAME="offline-conformance-fixture",
                       AIDA_USER="fixture-operator")
            output = root / "experiment"
            # Keep an idle terminal open: inherited stdin would leave init
            # waiting for an answer instead of reaching the managed-seat refusal.
            master, terminal = pty.openpty()
            try:
                self.assertTrue(os.isatty(terminal))
                result = c.subprocess.run([c.sys.executable, str(Path(c.__file__).resolve()),
                                           "--vendor", "codex", "--boundary", "aida", "--output", str(output)],
                                          env=env, stdin=terminal, stderr=terminal,
                                          # Authority records must be private even when the
                                          # invoking shell uses a collaborative umask.
                                          stdout=c.subprocess.PIPE, text=True, timeout=45,
                                          umask=0o077)
                os.set_blocking(master, False)
                diagnostics = bytearray()
                while True:
                    try:
                        chunk = os.read(master, 65536)
                    except BlockingIOError:
                        break
                    if not chunk:
                        break
                    diagnostics.extend(chunk)
                stderr = diagnostics.decode(errors="replace")
            finally:
                os.close(terminal)
                os.close(master)
            self.assertEqual(result.returncode, 2, result.stdout + stderr)
            self.assertIn("fixture-role-enter-refused-managed", stderr)
            self.assertIn("AIDA-managed agent sessions cannot issue direct TTY grants", stderr)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["boundary"], "aida")
            self.assertEqual(report["vendor_version"], "fixture-codex")
            self.assertIsNone(report["containment_receipt_sha256"])
            self.assertFalse(report["production_launcher_credit"])
            self.assertFalse(report["whole_lane_pass"])
            self.assertEqual({r["status"] for r in report["checks"].values()}, {"unknown"})
            self.assertEqual([r["stage"] for r in report["setup"]][-3:],
                             ["roster-allow-seat", "roster-allow-seat", "role-enter"])
            self.assertEqual(report["setup"][-3]["exit_code"], 0)
            self.assertEqual(report["setup"][-3]["argv"][-2:], ["fixture-operator", "advisor"])
            self.assertEqual(report["setup"][-1]["argv"][1:4], ["role", "enter", "advisor"])
            self.assertEqual(report["setup"][-2]["exit_code"], 0)
            self.assertEqual(report["setup"][-2]["argv"][-2:], ["fixture-operator", "implementer"])
            self.assertEqual(report["unsupported_reason"], "fixture-role-enter-refused-managed")
            self.assertIn("AIDA-managed agent sessions cannot issue direct TTY grants",
                          report["setup"][-1]["diagnostic"])
            self.assertNotEqual(report["setup"][-1]["exit_code"], 0)
            self.assertFalse(marker.exists())
            repo = output / "repository"
            policy = c.command(["git", "-C", str(repo), "show",
                                report["trusted_policy_sha"] + ":.aida/config.toml"])
            # Setup must produce valid TOML from the real init template, which
            # already defines [contained]. Verify the committed launch policy.
            # trace:BUG-2470 | ai:codex
            parsed = tomllib.loads(policy)
            self.assertTrue(parsed["agents"]["contained"])
            self.assertEqual(parsed["agents"]["vendor"], "codex")
            self.assertEqual(parsed["contained"]["vendors"]["codex"], "aida")
            self.assertEqual(parsed["contained"]["allowed_hosts"],
                             ["api.openai.com", "allow.fixture.test"])
            self.assertTrue((repo / ".aida-store/objects").is_dir())
            roster = (repo / ".aida-store/registry/team.toml").read_text()
            self.assertIn("fixture-operator", roster)
            self.assertIn("implementer", roster)
            self.assertIn("advisor", roster)
            self.assertTrue((output / "authority-home/.aida/roles/implementer.toml").is_file())
            self.assertEqual(report["aida_binary_sha256"], c.hashlib.sha256(Path(aida).resolve().read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
