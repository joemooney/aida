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
