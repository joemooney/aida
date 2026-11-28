"""Shared teardown for Python contract tests that create temp AIDA projects.

A stale read can detach a `cache refresh --worker` (cwd = the project root,
`--store`/`--cache` under it) that outlives the CLI call. Removing the fixture
while it still writes `.aida/cache-db` fails with ENOTEMPTY, so `temp_project`
waits for every fixture process before removing the tree.
trace:TASK-3962 | ai:claude
"""
import contextlib
import os
from pathlib import Path
import tempfile
import time


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
    spawned) exit before the tree is removed. Returns the processes seen at
    the start of the wait. trace:BUG-1990 | ai:claude"""
    if not os.path.isdir("/proc"):
        return []
    start = time.monotonic()
    first = live = fixture_processes(temp)
    while live:
        if time.monotonic() - start > timeout:
            raise AssertionError(f"fixture processes still running after {timeout}s: {live}")
        time.sleep(0.05)
        live = fixture_processes(temp)
    if first:
        print(f"teardown waited {(time.monotonic() - start) * 1000:.0f}ms for {first}", flush=True)
    return first


@contextlib.contextmanager
def temp_project(prefix):
    """`tempfile.TemporaryDirectory` that waits for fixture processes before
    removal, even when the body raised. trace:TASK-3962 | ai:claude"""
    temp = tempfile.TemporaryDirectory(prefix=prefix)
    try:
        yield temp.name
    finally:
        try:
            wait_for_fixture_processes(temp.name)
        finally:
            temp.cleanup()
