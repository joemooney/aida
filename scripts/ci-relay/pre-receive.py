#!/usr/bin/env python3
"""Server-side CI credential boundary. trace:STORY-2480 | ai:codex

Install on the receiving host, outside client control. A trusted forced-command
wrapper must supply AIDA_CI_PRINCIPAL=request|return|mirror for each SSH key.
Never accept that variable from SSH AcceptEnv or a push option. Unknown identity
refuses. A hosting platform must provide equivalent per-identity custom hooks;
protected-branch wildcards alone do not constrain arbitrary branch/tag creation.
"""
import os
import re
import selectors
import signal
import time
import subprocess
import sys


def reject():
    sys.stderr.write("CI credential/ref policy refused\n")
    raise SystemExit(1)


deadline = time.monotonic() + 25


def git(*args, cap=8 * 1024 * 1024, allowed=(0,)):
    process = subprocess.Popen(["git", *args], stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, start_new_session=True)
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    output = bytearray()
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                reject()
            events = selector.select(min(remaining, 1))
            for key, _ in events:
                chunk = os.read(key.fd, min(65536, cap + 1 - len(output)))
                if not chunk:
                    selector.unregister(key.fileobj)
                output.extend(chunk)
                if len(output) > cap:
                    reject()
        if process.wait(timeout=max(0.01, deadline - time.monotonic())) not in allowed:
            reject()
        return bytes(output)
    finally:
        selector.close()
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=2)
        process.stdout.close()


def intake_history(new):
    commits = git("rev-list", "--reverse", "--topo-order", "--parents", new).decode().splitlines()
    if not commits or len(commits) > 4096:
        reject()
    previous, parent = {}, None
    path_pattern = re.compile(r"(?:observations|receipts)/[0-9a-f]{64}\.json|ci-results/(?:[0-9a-f]{40}|[0-9a-f]{64})/[1-9][0-9]{0,9}\.json")
    identity = re.compile(r"(?:author|committer) AIDA Intake <intake@invalid> [0-9]+ [+-][0-9]{4}")
    for line in commits:
        fields = line.split()
        if fields[1:] != ([] if parent is None else [parent]):
            reject()
        commit = git("cat-file", "commit", fields[0], cap=4096).decode()
        headers, message = commit.split("\n\n", 1)
        lines = headers.splitlines()
        if message != "Record reviewed intake data\n" or len(lines) != (3 if parent is None else 4):
            reject()
        if sum(bool(identity.fullmatch(header)) for header in lines) != 2:
            reject()
        current = {}
        for entry in git("ls-tree", "-rz", "--full-tree", fields[0]).split(b"\0"):
            if not entry:
                continue
            metadata, path = entry.decode().split("\t", 1)
            mode, kind, oid = metadata.split()
            if mode != "100644" or kind != "blob" or not path_pattern.fullmatch(path):
                reject()
            if path.startswith("ci-results/") and int(path.rsplit("/", 1)[1][:-5]) > 4294967295:
                reject()
            current[path] = oid
        if not current or len(current) > 8192 or any(current.get(path) != oid for path, oid in previous.items()):
            reject()
        previous, parent = current, fields[0]


principal = os.environ.get("AIDA_CI_PRINCIPAL", "")
rows = sys.stdin.read(65537)
if len(rows) > 65536 or principal not in {"request", "return", "mirror"}:
    reject()
try:
    for row in rows.splitlines():
        fields = row.split()
        if len(fields) != 3:
            reject()
        old, new, ref = fields
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", new) or set(new) == {"0"}:
            reject()
        if principal in {"request", "mirror"}:
            prefix = "refs/aida/ci/" if principal == "request" else "refs/heads/ci/"
            number = ref.removeprefix(prefix)
            if not ref.startswith(prefix) or not re.fullmatch(r"[1-9][0-9]{0,9}", number) or int(number) > 4294967295:
                reject()
            if set(old) != {"0"} and old != new:
                reject()
            if git("cat-file", "-t", new, cap=64).strip() != b"commit":
                reject()
        elif ref != "refs/heads/aida-intent":
            reject()
        else:
            if set(old) != {"0"}:
                git("merge-base", "--is-ancestor", old, new, cap=64)
            intake_history(new)
except (OSError, ValueError, UnicodeError, subprocess.TimeoutExpired):
    reject()
