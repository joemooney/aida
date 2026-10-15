#!/usr/bin/env python3
"""Repro and detection tests for BUG-2135: sccache server with deleted TMPDIR.

Tests that missing sccache server temp directories are detected from process
environments, reported with a clear restart hint on gate failure, and repaired
before gates run.

trace:BUG-2135 | ai:antigravity
"""

import importlib.util
import os
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(REPO / "scripts"))

spec = importlib.util.spec_from_file_location(
    "check_ci_gates", REPO / "scripts" / "check-ci-gates.py"
)
check_ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_ci)


def test_missing_tmpdir_detected():
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

        old_env = os.environ.get("AIDA_SCCACHE_PROC_DIR")
        os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
        try:
            detected = check_ci.sccache_missing_tmpdir()
            assert detected == str(deleted_tmp), f"expected {deleted_tmp}, got {detected}"
        finally:
            if old_env is not None:
                os.environ["AIDA_SCCACHE_PROC_DIR"] = old_env
            else:
                os.environ.pop("AIDA_SCCACHE_PROC_DIR", None)
    print("ok - sccache missing TMPDIR detected from proc environ")


def test_valid_tmpdir_ignored():
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

        old_env = os.environ.get("AIDA_SCCACHE_PROC_DIR")
        os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
        try:
            detected = check_ci.sccache_missing_tmpdir()
            assert detected is None, f"expected None, got {detected}"
        finally:
            if old_env is not None:
                os.environ["AIDA_SCCACHE_PROC_DIR"] = old_env
            else:
                os.environ.pop("AIDA_SCCACHE_PROC_DIR", None)
    print("ok - valid sccache TMPDIR is not flagged")


def test_ensure_sccache_server_restarts_broken():
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

        env_backup = dict(os.environ)
        os.environ["AIDA_SCCACHE_PROC_DIR"] = str(proc)
        os.environ["AIDA_SCCACHE"] = str(fake_sccache)
        os.environ["AIDA_SCCACHE_TMPDIR"] = str(td)
        try:
            check_ci.ensure_sccache_server()
            log_contents = log.read_text()
            assert "--stop-server" in log_contents, f"stop-server missing: {log_contents}"
            assert "--start-server" in log_contents, f"start-server missing: {log_contents}"
        finally:
            os.environ.clear()
            os.environ.update(env_backup)
    print("ok - ensure_sccache_server stops and restarts broken server")


def main():
    test_missing_tmpdir_detected()
    test_valid_tmpdir_ignored()
    test_ensure_sccache_server_restarts_broken()
    print("All BUG-2135 sccache detection tests passed.")


if __name__ == "__main__":
    main()
