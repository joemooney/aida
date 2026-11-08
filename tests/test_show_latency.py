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
import shlex
import shutil
from pathlib import Path
import sqlite3
import tempfile
import time

from test_cache_refresh_outputs import require, run, run_tty


def fixture_processes(temp):
    """Live processes whose cwd, or any argv path (the worker carries
    --store/--cache under the fixture), is inside the fixture, as (pid, cmdline).
    Matching argv too keeps the wait effective if the worker cwd policy ever
    moves the cwd outside the fixture. trace:TASK-1357 | ai:claude"""
    roots = {os.path.realpath(temp), os.path.abspath(temp)}

    def inside(path):
        return any(path == r or path.startswith(r + os.sep) for r in roots)

    found = []
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            cwd = os.readlink(f"/proc/{pid}/cwd")
            raw = Path(f"/proc/{pid}/cmdline").read_bytes()
        except OSError:
            continue
        args = [a.decode(errors="replace") for a in raw.split(b"\0") if a]
        # An argv path may also be the value half of `--flag=/path`.
        paths = [a.split("=", 1)[-1] for a in args]
        if int(pid) != os.getpid() and (inside(cwd) or any(inside(a) for a in paths)):
            found.append((int(pid), " ".join(args)))
    return found


def wait_for_fixture_processes(temp, timeout=60):
    """Let the detached refresh worker (and any other child the fixture
    spawned) exit before TemporaryDirectory removes the tree under it. A
    worker still writing under .aida made rmtree fail with ENOTEMPTY after
    every check had passed. trace:BUG-1990 | ai:claude"""
    if not os.path.isdir("/proc"):
        return
    start = time.monotonic()
    first = live = fixture_processes(temp)
    while live:
        if time.monotonic() - start > timeout:
            raise AssertionError(f"fixture processes still running after {timeout}s: {live}")
        time.sleep(0.05)
        live = fixture_processes(temp)
    if first:
        print(f"teardown waited {(time.monotonic() - start) * 1000:.0f}ms for {first}", flush=True)


def check(binary):
    with tempfile.TemporaryDirectory(prefix="aida-show-latency-") as temp:
        try:
            check_in(binary, temp)
        finally:
            wait_for_fixture_processes(temp)


def check_in(binary, temp):
    root, home = Path(temp) / "project", Path(temp) / "home"
    root.mkdir()
    home.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_"))}
    env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
               GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
               GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
               AIDA_TELEMETRY="0", AIDA_CACHE_READ_WAIT_MS="1500")
    require(run(["git", "init", "-q", "-b", "main"], root, env))
    require(run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "commit", "--allow-empty", "-qm", "fixture"], root, env))
    # trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    git_dir = root / ".git"
    assert git_dir.is_dir() and not git_dir.is_symlink()
    assert not (git_dir / "commondir").exists()
    with (git_dir / "aida-allocation.toml").open("x", encoding="utf-8") as setting:
        setting.write("version=1\nauthoritative=true\n")
    # Trusted allocation evidence must not inherit group writes from the lane.
    # trace:BUG-2466 | ai:codex
    (git_dir / "aida-allocation.toml").chmod(0o600)
    require(run([binary, "init", "--no-skills", "--no-hooks"], root, env))
    require(run([binary, "add", "--title", "original title", "--type", "task"], root, env))
    require(run([binary, "cache", "rebuild"], root, env))
    cache, store = root / ".aida/cache-db/cache.db", root / ".aida-store"
    with sqlite3.connect(cache) as conn:
        uuid, spec = conn.execute("SELECT id, spec_id FROM requirements_cache WHERE title='original title'").fetchone()
    obj, = (store / "objects").rglob(f"{spec}.yaml")
    obj.write_text(obj.read_text().replace("original title", "canonical title"))
    require(run(["git", "add", "objects"], store, env))
    # trace:TASK-1955 | ai:claude (TASK-2010)
    require(run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "commit", "-qm", "external title"], store, env))
    # A foreign refresher cannot block show's pinned stale projection.
    # The requested object remains authoritative.
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
                assert metadata["refreshing"] == "deferred"
        for handle, flags in ((spec, ["--card"]), (uuid, []), (spec, ["--tree"])):
            start = time.monotonic()
            result = require(run_tty([binary, "show", handle, *flags, "--no-git", "--format", "human"], root, env))
            elapsed = time.monotonic() - start
            print(f"show {flags or handle}, held refresh lock: {elapsed * 1000:.0f}ms", flush=True)
            assert elapsed < 1, (handle, flags, elapsed)
            assert "canonical title" in result.stdout
    # trace:BUG-2466 | ai:codex
    # Show does not file refresh hints, even when their writer lock is held.
    wait_for_fixture_processes(temp)
    with sqlite3.connect(cache) as conn:
        conn.execute("UPDATE cache_meta SET value='' WHERE key='source_head_sha'")
    with open(str(cache) + ".refresh-request.lock", "a") as holder:
        fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for fmt in ("toon", "json", "human"):
            start = time.monotonic()
            result = require(run([binary, "show", spec, "--no-git", "--format", fmt], root, env, timeout=6))
            elapsed = time.monotonic() - start
            print(f"show {fmt}, held request lock: {elapsed * 1000:.0f}ms", flush=True)
            assert elapsed < 2, (fmt, elapsed, result.stderr)
            assert "canonical title" in result.stdout
            assert "note: showing results cached at" in result.stderr
            if fmt == "json":
                assert json.loads(result.stdout)["cache"]["refreshing"] == "deferred"
    # Legacy HOME clones still open FileDispenser with a blocking exclusive
    # lock. Reads do not allocate; hold it even with a fresh cache.
    require(run([binary, "cache", "refresh"], root, env))
    with open(store / ".aida/dispenser.lock", "a") as holder:
        fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = require(run([binary, "show", spec, "--no-git", "--json"], root, env, timeout=6))
        assert json.loads(result.stdout)["title"] == "canonical title"
    # A strict refresh may have started a worker in its constructor. Let it
    # finish before creating the next deliberately stale snapshot.
    wait_for_fixture_processes(temp)
    # Even a missing freshness stamp and a live SQLite writer must not
    # trigger a refresh, worker spawn or on-disk projection write.
    with sqlite3.connect(cache) as conn:
        conn.execute("UPDATE cache_meta SET value='' WHERE key='source_head_sha'")
    for suffix in (".refresh-request", ".refresh.log"):
        Path(str(cache) + suffix).unlink(missing_ok=True)
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
        assert not Path(str(cache) + ".refresh-request").exists()
        assert not Path(str(cache) + ".refresh.log").exists()
        assert writer.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()[0] == ""
    finally:
        writer.rollback()
        writer.close()
    # Releasing the holder still allows an ordinary strict operation to
    # bring the projection current; the no-wait show policy is scoped.
    require(run([binary, "cache", "refresh"], root, env))
    result = require(run([binary, "show", spec, "--no-git", "--json"], root, env))
    assert json.loads(result.stdout)["cache"]["stale"] is False
    # trace:BUG-2466 | ai:codex — the optional log must not consume the
    # response budget. Each show subprocess gets its own PATH, leaving the
    # fixture setup and the parent environment untouched.
    shim = root / "slow-git"
    shim.mkdir()
    real_git = shutil.which("git", path=env["PATH"])
    assert real_git
    script = shim / "git"
    calls = shim / "calls"
    script.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$*\" >> {shlex.quote(str(calls))}\n"
                      "for arg in \"$@\"; do\n"
                      "  case \"$arg\" in log|merge-base) sleep 3; break ;; esac\ndone\n"
                      f"exec {shlex.quote(real_git)} \"$@\"\n")
    script.chmod(0o755)
    slow_env = dict(env, PATH=str(shim) + os.pathsep + env["PATH"])
    # Default reads must never walk Git history, even when a stale cache
    # would previously refresh inline. Positive controls below prove the shim.
    with sqlite3.connect(cache) as conn:
        old_head = conn.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()[0]
    metadata_file = store / "metadata.yaml"
    metadata_file.write_text(metadata_file.read_text() + "\n")
    require(run(["git", "add", "metadata.yaml"], store, env))
    require(run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "commit", "-qm", "external metadata"], store, env))
    for flags in ([], ["--tree"], ["--no-git"], ["--format", "toon"], ["--format", "human"], ["--card", "--format", "human"]):
        calls.unlink(missing_ok=True)
        result = require(run([binary, "show", spec, *flags], root, slow_env, timeout=6))
        assert "canonical title" in result.stdout
        git_calls = calls.read_text() if calls.exists() else ""
        assert not any(set(line.split()) & {"log", "merge-base", "diff"} for line in git_calls.splitlines()), git_calls
        with sqlite3.connect(cache) as conn:
            assert conn.execute("SELECT value FROM cache_meta WHERE key='source_head_sha'").fetchone()[0] == old_head
    for flags in (["--full", "--format", "toon"], ["--json"]):
        calls.unlink(missing_ok=True)
        require(run([binary, "show", spec, *flags], root, slow_env, timeout=6))
        assert any("log" in line.split() for line in calls.read_text().splitlines()), calls.read_text()
    start = time.monotonic()
    result = require(run([binary, "show", spec, "--json"], root, slow_env, timeout=6))
    elapsed = time.monotonic() - start
    print(f"show json, slow Git log: {elapsed * 1000:.0f}ms", flush=True)
    assert elapsed < 2, (elapsed, result.stderr)
    linkage = json.loads(result.stdout)["git_linkage"]
    assert linkage["complete"] is False, linkage
    assert linkage["incomplete"], linkage
    assert linkage["shipped"] is None, linkage
    print("PASS show latency, canonical object, cache labels, strict refresh", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aida", type=Path, required=True)
    check(parser.parse_args().aida.resolve())
