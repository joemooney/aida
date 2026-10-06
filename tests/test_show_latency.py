#!/usr/bin/env python3
"""BUG-1801: show must not spend its one-second budget waiting for refresh.

Run: python3 tests/test_show_latency.py --aida /path/to/aida
Uses only isolated store/cache/home fixtures; no production lock or cache edits.
trace:BUG-1801 | ai:codex
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import time

from test_cache_refresh_outputs import require, run, run_tty


def check(binary):
    with tempfile.TemporaryDirectory(prefix="aida-show-latency-") as temp:
        root, home = Path(temp) / "project", Path(temp) / "home"
        root.mkdir()
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_"))}
        env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                   GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
                   GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
                   AIDA_TELEMETRY="0", AIDA_CACHE_READ_WAIT_MS="1500")
        require(run(["git", "init", "-q"], root, env))
        require(run([binary, "init", "--no-skills", "--no-hooks"], root, env))
        require(run([binary, "add", "--title", "original title", "--type", "task"], root, env))
        require(run([binary, "cache", "rebuild"], root, env))
        cache, store = root / ".aida/cache.db", root / ".aida-store"
        with sqlite3.connect(cache) as conn:
            uuid, spec = conn.execute("SELECT id, spec_id FROM requirements_cache WHERE title='original title'").fetchone()
        obj, = (store / "objects").rglob(f"{spec}.yaml")
        obj.write_text(obj.read_text().replace("original title", "canonical title"))
        require(run(["git", "add", "objects"], store, env))
        require(run(["git", "commit", "-qm", "external title"], store, env))
        # A live foreign refresher forces the default 1500ms wait in the old
        # binary, even for a one-object store. Show reads the canonical object
        # while labeling the old cache used for its derived graph context.
        with open(str(cache) + ".refresh.lock", "a") as holder:
            fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for fmt in ("toon", "json", "human"):
                start = time.monotonic()
                runner = run_tty if fmt == "human" else run
                result = require(runner([binary, "show", spec, "--no-git", "--format", fmt], root, env))
                elapsed = time.monotonic() - start
                print(f"show {fmt}, held refresh lock: {elapsed * 1000:.0f}ms", flush=True)
                assert elapsed < 1, (fmt, elapsed, result.stderr)
                assert "canonical title" in result.stdout
                assert "note: showing results cached at" in result.stderr
                if fmt == "json":
                    metadata = json.loads(result.stdout)["cache"]
                    assert metadata["stale"] is True
                    assert metadata["refreshing"] == "worker_running"
            for handle, flags in ((spec, ["--card"]), (uuid, []), (spec, ["--tree"])):
                start = time.monotonic()
                result = require(run_tty([binary, "show", handle, *flags, "--no-git", "--format", "human"], root, env))
                elapsed = time.monotonic() - start
                print(f"show {flags or handle}, held refresh lock: {elapsed * 1000:.0f}ms", flush=True)
                assert elapsed < 1, (handle, flags, elapsed)
                assert "canonical title" in result.stdout
        # A TTY winner with an unusable freshness stamp delegates a full
        # rebuild. Hold only this fixture's SQLite writer so the worker cannot
        # finish before we observe the stale result; release it before strict
        # recovery so no worker outlives the isolated fixture.
        with sqlite3.connect(cache) as conn:
            conn.execute("UPDATE cache_meta SET value='' WHERE key='source_head_sha'")
        writer = sqlite3.connect(cache)
        writer.execute("BEGIN IMMEDIATE")
        try:
            start = time.monotonic()
            result = require(run_tty([binary, "show", uuid, "--no-git", "--format", "human"], root, env))
            elapsed = time.monotonic() - start
            print(f"show human, full rebuild needed: {elapsed * 1000:.0f}ms", flush=True)
            assert elapsed < 1, elapsed
            assert "canonical title" in result.stdout
            assert "note: showing results cached at" in result.stderr
            assert Path(str(cache) + ".refresh-request").exists()
            assert Path(str(cache) + ".refresh.log").exists()
            assert writer.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()[0] == ""
        finally:
            writer.rollback()
            writer.close()
        # Releasing the holder still allows an ordinary strict operation to
        # bring the projection current; the no-wait show policy is scoped.
        require(run([binary, "cache", "refresh"], root, env))
        result = require(run([binary, "show", spec, "--no-git", "--json"], root, env))
        assert json.loads(result.stdout)["cache"]["stale"] is False
        print("PASS show latency, canonical object, cache labels, strict refresh", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aida", type=Path, required=True)
    check(parser.parse_args().aida.resolve())
