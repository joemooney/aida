"""BUG-2788: the live-bwrap watchdog kills and names a hanging test.

A fake libtest binary stands in for the cargo-built one: `--list` prints its
tests, `--exact <name>` runs one. `hangs_forever` sleeps forever and leaves a
grandchild `sleep` in its process group. trace:BUG-2788 | ai:claude
"""
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "ci-live-test-watchdog.py"

FAKE = r'''#!/usr/bin/env python3
import os, subprocess, sys, time
TESTS = {"mod::quick_ok": "ok", "mod::fails": "fail", "mod::hangs_forever": "hang"}
IGNORED = ["mod::ignored_one"]
args = sys.argv[1:]
if "--list" in args:
    names = IGNORED if "--ignored" in args else list(TESTS) + IGNORED
    filters = [a for a in args if not a.startswith("--") and a != "terse"]
    for n in names:
        if not filters or any(f in n for f in filters):
            print(n + ": test")
    sys.exit(0)
name = args[args.index("--exact") + 1]
print("running " + name, flush=True)
kind = TESTS[name]
if kind == "hang":
    child = subprocess.Popen(["sleep", "1000"])
    open(os.path.join(os.path.dirname(sys.argv[0]), "grandchild.pid"), "w").write(str(child.pid))
    print("SKIP? no: stalled inside bwrap", flush=True)
    time.sleep(10**6)
if kind == "fail":
    print("test result: FAILED. 0 passed; 1 failed;")
    sys.exit(101)
print("test result: ok. 1 passed; 0 failed;")
'''


class WatchdogTest(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp())
        self.binary = self.dir / "fake-test-bin"
        self.binary.write_text(FAKE)
        self.binary.chmod(0o755)
        self.log = self.dir / "live.log"
        self.results = self.dir / "live.results"
        self.pid_file = self.dir / "grandchild.pid"

    def run_watchdog(self, *extra):
        cmd = [sys.executable, str(SCRIPT), "--binary", str(self.binary), "--log", str(self.log),
               "--results", str(self.results), *extra, "mod::"]
        started = time.monotonic()
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, timeout=60)
        return proc, time.monotonic() - started

    def assert_grandchild_killed(self):
        pid = int(self.pid_file.read_text())
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return
            # A killed but unreaped child of an exited parent is reparented and reaped.
            stat = pathlib.Path(f"/proc/{pid}/stat")
            if stat.exists() and stat.read_text().split()[2] == "Z":
                return
            time.sleep(0.1)
        self.fail(f"grandchild sleep {pid} survived the kill")

    def test_hanging_test_is_killed_and_named_within_its_timeout(self):
        proc, elapsed = self.run_watchdog("--test-timeout", "2", "--overall-timeout", "30", "--jobs", "3")
        out = proc.stdout
        self.assertEqual(proc.returncode, 1, out)
        self.assertLess(elapsed, 15, out)
        self.assertIn("::error::test mod::hangs_forever exceeded its 2s timeout; killed", out)
        self.assertRegex(out, r"watchdog: running tests: .*mod::hangs_forever \(\d+s\)")
        # The log tail and the stalled test's own output are printed before failing.
        self.assertIn(f"lines of {self.log}:", out)
        self.assertIn("lines of mod::hangs_forever:\nrunning mod::hangs_forever", out)
        self.assertLess(out.index("exceeded its 2s timeout"), out.index("watchdog: 1 of 3 ok"))
        self.assertEqual(self.results.read_text().splitlines(), [
            "failed mod::fails", "timeout mod::hangs_forever",
            "ignored mod::ignored_one", "ok mod::quick_ok"])
        self.assertIn("--- mod::hangs_forever: timeout", self.log.read_text())
        self.assert_grandchild_killed()

    def test_overall_deadline_kills_running_and_names_the_unstarted(self):
        proc, elapsed = self.run_watchdog("--test-timeout", "60", "--overall-timeout", "2", "--jobs", "1")
        out = proc.stdout
        self.assertEqual(proc.returncode, 1, out)
        self.assertLess(elapsed, 15, out)
        self.assertRegex(out, r"::error::overall 2s deadline passed with 1 running and \d+ not started")
        self.assertIn("timeout mod::hangs_forever", self.results.read_text())
        self.assert_grandchild_killed()

    def test_all_passing_selection_exits_zero(self):
        cmd_filter = ["--test-timeout", "5"]
        proc = subprocess.run([sys.executable, str(SCRIPT), "--binary", str(self.binary), "--log",
                               str(self.log), "--results", str(self.results), *cmd_filter, "quick_ok"],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=30)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertEqual(self.results.read_text(), "ok mod::quick_ok\n")

    def test_zero_match_filter_fails(self):
        proc = subprocess.run([sys.executable, str(SCRIPT), "--binary", str(self.binary), "--log",
                               str(self.log), "--results", str(self.results), "no_such_test"],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=30)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("no tests match", proc.stdout)


class WorkflowStepTest(unittest.TestCase):
    def test_live_bwrap_step_has_a_15_minute_timeout_and_uses_the_watchdog(self):
        text = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        step = re.search(r"- name: Live bwrap OS-wrap confinement gate\n(.*?)\n      - name:", text, re.S).group(1)
        self.assertRegex(step, r"\n        timeout-minutes: 15\n")
        self.assertEqual(step.count("scripts/ci-live-test-watchdog.py"), 2)
        self.assertNotIn("cargo test -p", step)
        self.assertIn("AIDA_REQUIRE_BWRAP_LIVE=1 AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1", step)
        self.assertIn("SKIP (bwrap unavailable)", step)


if __name__ == "__main__":
    unittest.main()
