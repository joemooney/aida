#!/usr/bin/env python3
"""Timed full-status regression on an isolated generated 7k-row store.

Run with an optimized binary:
  python3 tests/test_status_cold_cache.py --aida target/agent/aida
For debug CI, use --cold-budget 30 --warm-budget 10.
No real store, cache, network credentials or user HOME is used.
trace:BUG-2481 | ai:codex
"""
import argparse
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import uuid


ROWS = 7000
EPICS = 1000


def run(args, root, env, timeout=30):
    result = subprocess.run([str(arg) for arg in args], cwd=root, env=env,
                            capture_output=True, text=True, timeout=timeout)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result


def check(binary, cold_budget=15, warm_budget=5):
    with tempfile.TemporaryDirectory(prefix="aida-status-cold-") as temp:
        root, home = Path(temp) / "project", Path(temp) / "home"
        root.mkdir()
        home.mkdir()
        env = dict(PATH="/usr/bin:/bin", LANG="C.UTF-8",
                   HOME=str(home), AIDA_HOME=str(home),
                   XDG_CONFIG_HOME=str(home / ".config"),
                   XDG_CACHE_HOME=str(home / ".cache"),
                   GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null",
                   GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
                   GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
                   AIDA_TELEMETRY="0", AIDA_OUTPUT_FORMAT="human")
        run(["git", "init", "-q"], root, env)
        run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0",
             "-c", "core.hooksPath=/dev/null", "commit", "-qm", "init",
             "--allow-empty"], root, env)
        run([binary, "init", "--no-skills", "--no-hooks", "--no-agent-config",
             "--no-roles"], root, env)
        store = root / ".aida-store"
        # Init may seed protocol/role specs. Keep the timed corpus exactly 7k.
        for path in (store / "objects").rglob("*.yaml"):
            path.unlink()
        ids = [str(uuid.UUID(int=i + 1)) for i in range(ROWS)]
        for i in range(ROWS):
            epic = i < EPICS
            kind, prefix = ("Epic", "EPIC") if epic else ("Task", "TASK")
            spec = f"{prefix}-{i + 1}"
            relationships = []
            # Nested epics expose repeated graph construction and per-child
            # linear UUID lookups, which flat unrelated tasks cannot detect.
            if epic and i + 1 < EPICS:
                relationships.append(dict(rel_type="Parent", target_id=ids[i + 1]))
            if not epic:
                relationships.append(dict(rel_type="Child", target_id=ids[i % EPICS]))
            obj = dict(id=ids[i], spec_id=spec, title=f"Generated item {i}",
                       description="Generated acceptance and implementation detail. " * 80,
                       status="Approved" if epic else "Completed", priority="Medium",
                       owner="", feature="Uncategorized", req_type=kind,
                       created_at="2026-01-01T00:00:00Z",
                       modified_at="2026-01-01T00:00:00Z", relationships=relationships)
            path = store / "objects" / prefix / f"{i // 1000:03}" / f"{spec}.yaml"
            path.parent.mkdir(parents=True, exist_ok=True)
            # JSON is valid YAML; this keeps the generated fixture stdlib-only.
            path.write_text(json.dumps(obj), encoding="utf-8")
        run(["git", "add", "objects"], store, env)
        run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0",
             "-c", "core.hooksPath=/dev/null", "commit", "-qm", "fixture"],
            store, env)
        cache = root / ".aida/cache-db/cache.db"
        for path in cache.parent.glob("cache.db*"):
            if path.is_file():
                path.unlink()
        assert not cache.exists()
        for label, budget in (("cold", cold_budget), ("warm", warm_budget)):
            print(f"Running {label} full status ({ROWS} rows, budget {budget}s)", flush=True)
            started = time.monotonic()
            # No remote is configured. Exercise full status including hygiene;
            # --no-ci avoids an unrelated external CI probe.
            result = run([binary, "status", "--full", "--no-ci", "--no-dev-context"],
                         root, env, timeout=budget)
            elapsed = time.monotonic() - started
            assert elapsed < budget, (label, elapsed, result.stdout, result.stderr)
            with sqlite3.connect(cache) as conn:
                count = conn.execute("SELECT COUNT(*) FROM requirements_cache").fetchone()[0]
                epics = dict(conn.execute(
                    "SELECT status, COUNT(*) FROM requirements_cache WHERE req_type='Epic' "
                    "GROUP BY status").fetchall())
            assert count == ROWS, (label, count)
            # Intermediate epics are stored Approved; their parents see that
            # open work even though every task is Completed. Only the leaf
            # epic has an entirely Completed subtree.
            assert epics == {"InProgress": EPICS - 1, "Completed": 1}, (label, epics)
            print(f"{label}: {elapsed:.2f}s; {count} cached rows; epics {epics}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aida", required=True, type=Path)
    parser.add_argument("--cold-budget", type=float, default=15,
                        help="cold status timeout in seconds (default: 15 for optimized builds)")
    parser.add_argument("--warm-budget", type=float, default=5,
                        help="warm status timeout in seconds (default: 5 for optimized builds)")
    args = parser.parse_args()
    check(args.aida.resolve(), args.cold_budget, args.warm_budget)
