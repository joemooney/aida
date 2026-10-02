#!/usr/bin/env python3
"""Pre-C cache-label CLI acceptance, using an isolated git store and HOME.

trace:TASK-1526 | ai:codex
Run after building: python3 tests/test_cache_refresh_outputs.py --aida target/debug/aida
MCP's same-call envelope/reset contract is covered by the Rust MCP seam test.
"""
import argparse
import fcntl
import json
import os
import pty
import select
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile
import termios
import time


def run(args, root, env, timeout=30):
    return subprocess.run([str(a) for a in args], cwd=root, env=env,
                          text=True, capture_output=True, timeout=timeout)


def run_tty(args, root, env):
    master, slave = pty.openpty()
    # An unsized pty makes width-sensitive human output truncate its columns, so
    # an assertion on a row's text would pass or fail depending on the host.
    # Give it a real window. trace:TASK-1526 | ai:claude
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
    child = subprocess.Popen([str(arg) for arg in args], cwd=root, env=env,
                             stdin=subprocess.DEVNULL, stdout=slave, stderr=subprocess.PIPE)
    os.close(slave)
    chunks = []
    try:
        while True:
            if not select.select([master], [], [], 30)[0]:
                child.kill()
                raise AssertionError(f"TTY command timed out: {args}")
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            chunks.append(chunk)
        _, stderr = child.communicate(timeout=30)
        return subprocess.CompletedProcess(args, child.returncode,
                                           b"".join(chunks).decode().replace("\r\n", "\n"),
                                           stderr.decode())
    finally:
        os.close(master)
        if child.poll() is None:
            child.kill(); child.wait()


def require(result):
    assert result.returncode == 0, (result.args, result.stdout, result.stderr)
    return result


def losing_reader_past_wait_prints_stale_note_and_json_flag_exit_0(result):
    assert result.returncode == 0
    metadata = json.loads(result.stdout)["cache"]
    assert metadata["stale"] is True and metadata["refreshing"] == "worker_running"
    assert result.stderr.count("note: showing results cached at") == 1


def stale_allowed_outputs_cover_deferred_writer_busy_and_worker_running(binary):
    with tempfile.TemporaryDirectory(prefix="aida-cache-labels-") as temp:
        root = Path(temp) / "project"
        home = Path(temp) / "home"
        root.mkdir(); home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_"))}
        env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                   GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
                   GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
                   AIDA_TELEMETRY="0", AIDA_CACHE_READ_WAIT_MS="20", AIDA_CACHE_RETRY_COUNT="0")
        require(run(["git", "init", "-q"], root, env))
        require(run([binary, "init", "--no-skills", "--no-hooks"], root, env))
        require(run([binary, "add", "--title", "cache-label-seed", "--type", "task", "--status", "draft"], root, env))
        require(run([binary, "cache", "rebuild"], root, env))
        cache = root / ".aida/cache.db"
        store = root / ".aida-store"
        with sqlite3.connect(cache) as conn:
            original_head = conn.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()[0]
            built_at = conn.execute("SELECT value FROM cache_meta WHERE key='built_at'").fetchone()[0]
            uuid, spec = conn.execute("SELECT id, spec_id FROM requirements_cache WHERE title='cache-label-seed'").fetchone()
        # One canonical commit without write-through, preserving the useful old row.
        files = list((store / "objects").rglob(f"{spec}.yaml"))
        assert len(files) == 1
        files[0].write_text(files[0].read_text().replace("cache-label-seed", "cache-label-new"))
        require(run(["git", "add", "objects"], store, env))
        require(run(["git", "commit", "-qm", "external title"], store, env))
        # Freeze the committed baseline using SQLite backup, never copying a live WAL file.
        baseline = Path(temp) / "baseline.db"
        with sqlite3.connect(cache) as source, sqlite3.connect(baseline) as dest:
            source.backup(dest)

        # Existing tolerant CLI routes. TASK-1514 owns expanding load/list_requirements
        # callers; show is separately owned by BUG-1674. Neither is converted here.
        surfaces = [
            ("list", ["list"], "array"),
            ("search", ["search", "cache-label"], "array"),
            ("status", ["status", "--no-dev-context"], "object"),
            ("queue list", ["queue", "list"], "array"),
            ("findings list", ["findings", "list"], "object"),
            ("advisor status", ["advisor", "status"], "object"),
            ("fasttrack status", ["fasttrack", "status"], "array"),
            ("ps", ["ps"], "object"),
        ]
        for state in ("deferred", "writer_busy", "worker_running"):
            for name, args, shape in surfaces:
                for output_format in ("json", "toon", "human"):
                    with sqlite3.connect(baseline) as source, sqlite3.connect(cache) as dest:
                        source.backup(dest)
                    with sqlite3.connect(cache) as conn:
                        if state == "deferred":
                            conn.execute("DELETE FROM cache_meta WHERE key='source_head_sha'")
                    writer = None
                    flock = None
                    if state == "writer_busy":
                        writer = sqlite3.connect(cache)
                        writer.execute("BEGIN IMMEDIATE")
                    if state == "worker_running":
                        flock = open(str(cache) + ".refresh.lock", "a")
                        fcntl.flock(flock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    try:
                        start = time.monotonic()
                        result = require(run([binary, *args, "--format", output_format], root, env))
                        elapsed = time.monotonic() - start
                        # Bound the ordinary single-query fixture, not the CPU
                        # work of dashboards aggregating independent sections.
                        # The core tests assert the shared polling deadline.
                        if name in ("list", "search"):
                            assert elapsed < 2, (name, state, output_format, elapsed)
                        elif elapsed >= 2:
                            print(f"TIMING {name} {state} {output_format}: {elapsed:.3f}s (whole dashboard)", flush=True)
                        notes = [line for line in result.stderr.splitlines() if line.startswith("note: showing results cached at")]
                        assert len(notes) == 1, (name, state, output_format, result.stdout, result.stderr)
                        if state == "deferred":
                            assert "refresh is deferred" in notes[0]
                            assert "few seconds" not in notes[0]
                        elif state == "writer_busy":
                            assert "write lock was busy" in notes[0]
                            assert "few seconds" not in notes[0]
                        else:
                            assert "cache is refreshing" in notes[0]
                        if output_format == "json":
                            value = json.loads(result.stdout)
                            assert isinstance(value, list if shape == "array" else dict), (name, value)
                            if shape == "object":
                                metadata = value["cache"]
                                assert metadata["stale"] is True
                                assert metadata["refreshing"] == state
                                assert metadata["cache_head"] == (None if state == "deferred" else original_head)
                                assert metadata["built_at"] == built_at
                                if state == "worker_running":
                                    losing_reader_past_wait_prints_stale_note_and_json_flag_exit_0(result)
                        elif output_format == "toon":
                            assert "cache:\n" in result.stdout
                            assert f'  refreshing: "{state}"' in result.stdout
                        assert not Path(str(cache) + ".refresh-request").exists()
                    finally:
                        if writer is not None: writer.rollback(); writer.close()
                        if flock is not None: flock.close()
            print(f"PASS {state}: JSON/TOON/human on {len(surfaces)} tolerant surfaces", flush=True)
        # These read surfaces still use strict load() today (TASK-1514 owns
        # expansion). Their object renderers nevertheless always carry cache.
        for name, args in [
            ("graph/tree", ["graph", spec, "--tree", "--format", "json"]),
            ("history", ["history", "--limit", "1", "--format", "json"]),
            ("digest", ["digest", "--digest-format", "json", "--format", "human"]),
        ]:
            with sqlite3.connect(baseline) as source, sqlite3.connect(cache) as dest:
                source.backup(dest)
            with sqlite3.connect(cache) as conn:
                conn.execute("DELETE FROM cache_meta WHERE key='source_head_sha'")
            result = require(run([binary, *args], root, env))
            value = json.loads(result.stdout)
            if isinstance(value, dict):
                assert "cache" in value, (name, value)
                if value["cache"]["stale"]:
                    assert "note: showing results cached at" in result.stderr
            else:
                assert isinstance(value, list), (name, value)
            print(f"PASS {name}: retained shape and cache contract", flush=True)

        # Advisory/no-output modes have no JSON object projection. They must
        # not wait on the flock or claim a background worker was requested.
        with sqlite3.connect(baseline) as source, sqlite3.connect(cache) as dest:
            source.backup(dest)
        with open(str(cache) + ".refresh.lock", "a") as flock:
            fcntl.flock(flock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for args in (["awaiting", "--notice"], ["statusline"]):
                start = time.monotonic()
                result = require(run([binary, *args], root, env))
                assert time.monotonic() - start < 2, args
                assert "requested" not in result.stderr
        print("PASS advisory surfaces: zero wait", flush=True)
        for mode in ("json", "toon", "human"):
            with sqlite3.connect(baseline) as source, sqlite3.connect(cache) as dest:
                source.backup(dest)
            with sqlite3.connect(cache) as conn:
                conn.execute("DELETE FROM cache_meta WHERE key='source_head_sha'")
            result = require(run_tty([binary, "list", "--format", mode], root, env))
            with sqlite3.connect(cache) as conn:
                recorded = conn.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()
            if mode == "human":
                assert recorded is not None and recorded[0] != original_head
                assert "cache-label-new" in result.stdout
                assert "refresh is deferred" not in result.stderr
            else:
                assert recorded is None, (mode, "machine output rebuilt at a TTY")
                assert "refresh is deferred" in result.stderr
        print("PASS tty_full_rebuild_winner_remains_strict_before_c: explicit JSON/TOON still defer at a TTY", flush=True)
        print("PASS losing_reader_past_wait_prints_stale_note_and_json_flag_exit_0", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--aida", type=Path, required=True)
    args = parser.parse_args()
    stale_allowed_outputs_cover_deferred_writer_busy_and_worker_running(args.aida.resolve())
