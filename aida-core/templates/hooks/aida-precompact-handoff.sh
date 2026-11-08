#!/usr/bin/env python3
"""Best-effort Claude PreCompact snapshot; persistence belongs to session handoff.

No transcript scraping, model calls, forge probes, or new state directory.
Every subprocess has a deadline and output cap; the entire hook has 12 seconds.
"""
# trace:STORY-2662 | ai:codex
import json
import os
import re
import selectors
import signal
import subprocess
import sys
import tempfile
import time

DEADLINE = time.monotonic() + 12


def run(argv, limit=2048, body=None):
    remaining = min(2, DEADLINE - time.monotonic())
    if remaining <= 0:
        return "unknown (hook budget exhausted)"
    source = tempfile.TemporaryFile() if body else None
    if source:
        source.write(body.encode())
        source.seek(0)
    child = subprocess.Popen(argv, stdin=source if source else subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             start_new_session=True)
    data = bytearray()
    try:
        end = time.monotonic() + remaining
        with selectors.DefaultSelector() as sel:
            sel.register(child.stdout, selectors.EVENT_READ)
            while time.monotonic() < end:
                if not sel.select(max(0, end - time.monotonic())):
                    break
                chunk = os.read(child.stdout.fileno(), min(1024, limit + 1 - len(data)))
                if not chunk:
                    rc = child.wait(timeout=max(.01, end - time.monotonic()))
                    return data.decode('utf-8', errors='replace').strip() if rc == 0 else 'unknown (command refused)'
                data.extend(chunk)
                if len(data) > limit:
                    return data[:limit].decode('utf-8', errors='ignore') + '\n[truncated]'
        return 'unknown (command timed out)'
    finally:
        # Also close descendants that inherited stdout after their parent exited.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        child.stdout.close()
        if source:
            source.close()


def main():
    raw = sys.stdin.buffer.read(65537)
    if len(raw) > 65536:
        return
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        return
    event = payload.get('hook_event_name')
    if event not in ('PreCompact', 'SessionStart'):
        return
    cwd = payload.get('cwd')
    if not isinstance(cwd, str) or not os.path.isabs(cwd):
        return
    os.chdir(cwd)
    seat = os.environ.get('AIDA_ROTATION_SEAT') or os.environ.get('AIDA_SESSION_ROLE')
    if not seat or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', seat):
        return
    handoff = ['aida', 'session', 'handoff', '--seat', seat]
    if event == 'SessionStart':
        # Compact resumes and relaunches both receive the existing substrate note.
        text = run(handoff + ['--show'], 8192)
        if text and not text.startswith('unknown ('):
            print(json.dumps({'hookSpecificOutput': {'hookEventName': event,
                            'additionalContext': text}}))
        return
    branch = run(['git', 'branch', '--show-current'], 256)
    spec = os.environ.get('AIDA_SESSION_SCOPE', '')
    # Branch fallback is a pointer only; never grants completion/ownership credit.
    if not spec:
        match = re.search(r'(?:STORY|TASK|BUG|SPIKE)-[A-Za-z0-9]+(?:-[0-9]+)?', branch, re.I)
        spec = match.group(0).upper() if match else 'unknown'
    spec = spec[:160]
    verdict = run(['aida', 'review', 'verdict', spec]) if spec != 'unknown' else 'unknown (no spec pointer)'
    findings = run(['aida', 'findings', 'list'])
    prior = run(handoff + ['--show'], 2048)
    body = (f'# PreCompact recovery snapshot (Claude; informational, revalidate before action)\n'
            f'Spec pointer: {spec}\nBranch: {branch}\n'
            f'Last verdict:\n{verdict}\nOpen findings (project-wide):\n{findings}\n'
            f'Prior handoff / next-action context:\n{prior}\n'
            f'Next step: inspect `aida show {spec}` and the working diff; reconcile the '
            'verdict and findings, then continue the active spec. Unknown evidence stays unknown.\n')
    # Safely below session handoff’s 16 KiB cap even with multibyte diagnostics.
    body = body.encode()[:10000].decode('utf-8', errors='ignore')
    result = run(handoff + ['--write', '-'], 1024, body)
    if result.startswith('unknown ('):
        print('AIDA PreCompact handoff unavailable: ' + result, file=sys.stderr)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError):
        # Optional recovery aid: failure must never block compaction or a lane.
        pass
