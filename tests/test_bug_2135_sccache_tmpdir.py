#!/usr/bin/env python3
"""Repro and detection tests for BUG-2135: sccache server with deleted TMPDIR.

Tests that missing sccache server temp directories are detected from process
environments, pre-started or restarted with a stable TMPDIR, reported with a
clear restart hint on gate failure, and respect RUSTC_WRAPPER opt-outs.

trace:BUG-2135 | ai:antigravity
"""

import importlib.util
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(REPO / "scripts"))

spec = importlib.util.spec_from_file_location(
    "check_ci_gates", REPO / "scripts" / "check-ci-gates.py"
)
check_ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_ci)


class TestSccacheTmpdirDetection(unittest.TestCase):
    def setUp(self):
        self.env_backup = dict(os.environ)

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.env_backup)

    def test_missing_tmpdir_detected(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()
            pid_dir = proc / "12345"
            pid_dir.mkdir()
            (pid_dir / "comm").write_text("sccache\n")

            deleted_tmp = Path(td) / "deleted_tmp"
            (pid_dir / "environ").write_bytes(
                b"FOO=1\0TMPDIR=" + str(deleted_tmp).encode() + b"\0BAR=2\0"
            )

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            detected = check_ci.sccache_missing_tmpdir()
            self.assertEqual(detected, str(deleted_tmp))

    def test_valid_tmpdir_ignored(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()
            pid_dir = proc / "12345"
            pid_dir.mkdir()
            (pid_dir / "comm").write_text("sccache\n")

            valid_tmp = Path(td) / "valid_tmp"
            valid_tmp.mkdir()
            (pid_dir / "environ").write_bytes(
                b"FOO=1\0TMPDIR=" + str(valid_tmp).encode() + b"\0BAR=2\0"
            )

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            detected = check_ci.sccache_missing_tmpdir()
            self.assertIsNone(detected)

    def test_ensure_sccache_server_restarts_broken(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()
            pid_dir = proc / "12345"
            pid_dir.mkdir()
            (pid_dir / "comm").write_text("sccache\n")

            deleted_tmp = Path(td) / "deleted_tmp"
            (pid_dir / "environ").write_bytes(
                b"TMPDIR=" + str(deleted_tmp).encode() + b"\0"
            )

            log = Path(td) / "sccache.log"
            fake_sccache = Path(td) / "fake_sccache"
            fake_sccache.write_text(f"#!/bin/bash\necho \"$*\" >> {log}\n")
            fake_sccache.chmod(0o755)

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            os.environ["AIDA_SCCACHE"] = str(fake_sccache)
            os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)
            os.environ.pop("RUSTC_WRAPPER", None)

            check_ci.ensure_sccache_server()
            self.assertTrue(log.exists())
            log_contents = log.read_text()
            self.assertIn("--stop-server", log_contents)
            self.assertIn("--start-server", log_contents)

    def test_ensure_sccache_server_prestarts_when_not_running(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()

            log = Path(td) / "sccache.log"
            fake_sccache = Path(td) / "fake_sccache"
            fake_sccache.write_text(f"#!/bin/bash\necho \"$*\" >> {log}\n")
            fake_sccache.chmod(0o755)

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            os.environ["AIDA_SCCACHE"] = str(fake_sccache)
            os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)
            os.environ.pop("RUSTC_WRAPPER", None)

            check_ci.ensure_sccache_server()
            self.assertTrue(log.exists())
            log_contents = log.read_text()
            self.assertNotIn("--stop-server", log_contents)
            self.assertIn("--start-server", log_contents)

    def test_ensure_sccache_server_noop_when_healthy(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()
            pid_dir = proc / "12345"
            pid_dir.mkdir()
            (pid_dir / "comm").write_text("sccache\n")

            valid_tmp = Path(td) / "valid_tmp"
            valid_tmp.mkdir()
            (pid_dir / "environ").write_bytes(
                b"TMPDIR=" + str(valid_tmp).encode() + b"\0"
            )

            log = Path(td) / "sccache.log"
            fake_sccache = Path(td) / "fake_sccache"
            fake_sccache.write_text(f"#!/bin/bash\necho \"$*\" >> {log}\n")
            fake_sccache.chmod(0o755)

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            os.environ["AIDA_SCCACHE"] = str(fake_sccache)
            os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)
            os.environ.pop("RUSTC_WRAPPER", None)

            check_ci.ensure_sccache_server()
            self.assertFalse(log.exists())

    def test_ensure_sccache_server_opt_out(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()

            log = Path(td) / "sccache.log"
            fake_sccache = Path(td) / "fake_sccache"
            fake_sccache.write_text(f"#!/bin/bash\necho \"$*\" >> {log}\n")
            fake_sccache.chmod(0o755)

            os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
            os.environ["AIDA_SCCACHE"] = str(fake_sccache)
            os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)

            # Case 1: RUSTC_WRAPPER=""
            os.environ["RUSTC_WRAPPER"] = ""
            check_ci.ensure_sccache_server()
            self.assertFalse(log.exists())

            # Case 2: RUSTC_WRAPPER="other-wrapper"
            os.environ["RUSTC_WRAPPER"] = "other-wrapper"
            check_ci.ensure_sccache_server()
            self.assertFalse(log.exists())

    def test_hint_format(self):
        expected_hint = "sccache --stop-server; TMPDIR=/tmp sccache --start-server"
        ci_script = (REPO / "scripts" / "check-ci-gates.py").read_text()
        self.assertIn(expected_hint, ci_script)

        rust_src = (REPO / "aida-cli-lib" / "src" / "dev_build.rs").read_text()
        self.assertIn(expected_hint, rust_src)


class TestTask2772SccacheFallback(unittest.TestCase):
    """TASK-2772: a refused or silent sccache falls back to RUSTC_WRAPPER= once.

    trace:TASK-2772 | ai:claude
    """

    def fake(self, td, body):
        script = Path(td) / "fake_sccache"
        script.write_text("#!/bin/bash\n" + body + "\n")
        script.chmod(0o755)
        return str(script)

    def test_refused_sccache_falls_back_with_one_note(self):
        with tempfile.TemporaryDirectory() as td:
            env = {"RUSTC_WRAPPER": "sccache", "AIDA_SCCACHE": self.fake(td, "exit 2")}
            out = check_ci.sccache_fallback(env)
            self.assertEqual(out["RUSTC_WRAPPER"], "")

    def test_silent_sccache_never_hangs(self):
        with tempfile.TemporaryDirectory() as td:
            env = {"AIDA_SCCACHE": self.fake(td, "sleep 30")}
            saved = check_ci.SCCACHE_PROBE_TIMEOUT
            check_ci.SCCACHE_PROBE_TIMEOUT = 0.5
            try:
                out = check_ci.sccache_fallback(env)
            finally:
                check_ci.SCCACHE_PROBE_TIMEOUT = saved
            self.assertEqual(out["RUSTC_WRAPPER"], "")

    def test_working_sccache_and_opt_outs_are_untouched(self):
        with tempfile.TemporaryDirectory() as td:
            env = {"RUSTC_WRAPPER": "sccache", "AIDA_SCCACHE": self.fake(td, "exit 0")}
            self.assertIs(check_ci.sccache_fallback(env), env)
        refuse = lambda: self.fail("an opted-out wrapper must not be probed")
        for env in ({"RUSTC_WRAPPER": ""}, {"RUSTC_WRAPPER": "other"}, {"AIDA_SCCACHE_DISABLE": "1"}):
            self.assertIs(check_ci.sccache_fallback(env, probe=refuse), env)


class TestTask2772BoundedServerStart(unittest.TestCase):
    """TASK-2772 (review F3): the runner's start path never waits on a stuck
    sccache: start/stop run under a deadline and a timeout falls back.

    trace:TASK-2772 | ai:claude
    """

    def setUp(self):
        self.saved_env = dict(os.environ)
        self.saved_timeouts = (check_ci.SCCACHE_SERVER_TIMEOUT, check_ci.SCCACHE_PROBE_TIMEOUT)

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.saved_env)
        check_ci.SCCACHE_SERVER_TIMEOUT, check_ci.SCCACHE_PROBE_TIMEOUT = self.saved_timeouts

    def silent_sccache(self, td):
        script = Path(td) / "fake_sccache"
        script.write_text("#!/bin/bash\nexec sleep 100000\n")
        script.chmod(0o755)
        return str(script)

    def run_start_path(self, td, proc):
        os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
        os.environ["AIDA_SCCACHE"] = self.silent_sccache(td)
        os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)
        os.environ.pop("RUSTC_WRAPPER", None)
        os.environ.pop("AIDA_SCCACHE_DISABLE", None)
        check_ci.SCCACHE_SERVER_TIMEOUT = 0.5
        check_ci.SCCACHE_PROBE_TIMEOUT = 0.5
        started = time.monotonic()
        env = check_ci.gate_env()
        return env, time.monotonic() - started

    def test_silent_sccache_start_falls_back_within_bound(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            proc.mkdir()  # no server running: the --start-server path
            env, elapsed = self.run_start_path(td, proc)
            self.assertEqual(env["RUSTC_WRAPPER"], "")
            self.assertLess(elapsed, 5.0)

    def test_silent_sccache_restart_falls_back_within_bound(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "proc"
            pid_dir = proc / "12345"
            pid_dir.mkdir(parents=True)
            (pid_dir / "comm").write_text("sccache\n")
            gone = Path(td) / "gone"
            (pid_dir / "environ").write_bytes(b"TMPDIR=" + str(gone).encode() + b"\0")
            env, elapsed = self.run_start_path(td, proc)  # the stale-TMPDIR restart
            self.assertEqual(env["RUSTC_WRAPPER"], "")
            self.assertLess(elapsed, 5.0)

    def test_server_command_timeout_reports_failure(self):
        with tempfile.TemporaryDirectory() as td:
            check_ci.SCCACHE_SERVER_TIMEOUT = 0.5
            self.assertFalse(check_ci._sccache_server_cmd([self.silent_sccache(td), "--start-server"]))
            self.assertFalse(check_ci._sccache_server_cmd([str(Path(td) / "absent")]))
            self.assertTrue(check_ci._sccache_server_cmd(["true"]))


class TestTask2772ProbesCargosWrapper(unittest.TestCase):
    """TASK-2772: the probe runs the sccache cargo will use (its configured
    `build.rustc-wrapper`), not whichever sccache is first on PATH.

    trace:TASK-2772 | ai:claude
    """

    def test_configured_wrapper_wins_over_path(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            refused = root / "cargo-home" / "bin" / "sccache"
            refused.parent.mkdir(parents=True)
            refused.write_text("#!/bin/bash\nexit 2\n")
            refused.chmod(0o755)
            (root / "cargo-home" / "config.toml").write_text(
                f'[build]\nrustc-wrapper = "{refused}"\n'
            )
            shim_dir = root / "shim"
            shim_dir.mkdir()
            (shim_dir / "sccache").write_text("#!/bin/bash\nexit 0\n")
            (shim_dir / "sccache").chmod(0o755)
            env = {"CARGO_HOME": str(root / "cargo-home"), "PATH": str(shim_dir)}
            self.assertEqual(check_ci.configured_rustc_wrapper(env, cwd=root), str(refused))
            self.assertEqual(check_ci.sccache_probe_bin(env, cwd=root), str(refused))
            # The nearest project config overrides CARGO_HOME's.
            (root / "proj" / ".cargo").mkdir(parents=True)
            (root / "proj" / ".cargo" / "config.toml").write_text(
                '[build]\nrustc-wrapper = "/opt/other-sccache"\n'
            )
            self.assertEqual(
                check_ci.configured_rustc_wrapper(env, cwd=root / "proj"), "/opt/other-sccache"
            )


class TestBug2418CargoHomeNote(unittest.TestCase):
    """BUG-2418: a read-only CARGO_HOME maps to one line.

    trace:BUG-2418 | ai:claude
    """

    def test_writable_or_missing_home_has_no_note(self):
        with tempfile.TemporaryDirectory() as td:
            self.assertIsNone(check_ci.cargo_home_read_only_note({"CARGO_HOME": td}))
            missing = str(Path(td) / "absent")
            self.assertIsNone(check_ci.cargo_home_read_only_note({"CARGO_HOME": missing}))

    @unittest.skipIf(os.geteuid() == 0, "root ignores directory write bits")
    def test_unwritable_home_maps_to_the_fetch_note(self):
        with tempfile.TemporaryDirectory() as td:
            os.chmod(td, 0o555)
            try:
                note = check_ci.cargo_home_read_only_note({"CARGO_HOME": td})
            finally:
                os.chmod(td, 0o755)
            self.assertEqual(
                note, "new dependencies are fetched on the host: run cargo fetch outside the lane"
            )


if __name__ == "__main__":
    unittest.main()
