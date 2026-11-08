#!/usr/bin/env python3
"""Best-effort Claude PreCompact snapshot; persistence belongs to session handoff.

No transcript scraping, model calls, or new state directory.
Every subprocess has a deadline and output cap; the entire hook has 14 seconds.
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
from datetime import datetime, timezone

DEADLINE = time.monotonic() + 14


def run(argv, limit=2048, body=None, timeout=3):
    remaining = min(timeout, DEADLINE - time.monotonic() - (0 if body else 1.5))
    if remaining <= 0:
        return "unknown (hook budget exhausted)"
    source = tempfile.TemporaryFile() if body else None
    if source:
        source.write(body.encode())
        source.seek(0)
    try:
        child = subprocess.Popen(argv, stdin=source if source else subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
    except OSError:
        if source:
            source.close()
        return 'unknown (command unavailable)'
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
    sid = payload.get('session_id')
    if not isinstance(sid, str) or not sid or len(sid) > 256:
        return
    # CLI owns a reusable worktree/role slot in the existing handoff substrate.
    handoff = ['aida', 'session', 'handoff', '--seat', seat]
    if event == 'SessionStart':
        if payload.get('source') != 'compact':
            return
        text = run(handoff + ['--snapshot', '--show', '--body-only'], 12000)
        if text and not text.startswith('unknown ('):
            print(json.dumps({'hookSpecificOutput': {'hookEventName': event,
                            'additionalContext': 'Recovery snapshot; may predate the latest compaction if its write failed. Revalidate before acting.\n' + text}}))
        return
    branch = run(['git', 'branch', '--show-current'], 256)
    spec = os.environ.get('AIDA_SESSION_SCOPE', '')
    spec_known = bool(spec)
    # Branch fallback is a pointer only; never grants completion/ownership credit.
    if not spec:
        match = re.search(r'(?<![A-Za-z0-9])(?:STORY|TASK|BUG|SPIKE)-[A-Za-z0-9]+(?:-[0-9]+)?', branch, re.I)
        spec = match.group(0).upper() if match else 'unknown'
    spec = spec[:160]
    verdict = run(['aida', 'review', 'verdict', spec]) if spec != 'unknown' else 'unknown (no spec pointer)'
    # Branch-scoped GitHub read: never infer absence from a fleet-wide page.
    # Other forges remain explicitly unknown; compaction is optional.
    # A failed lookup must not turn PR-keyed review findings into an empty set.
    prs = run(['gh', 'pr', 'list', '--head', branch, '--state', 'open',
               '--limit', '100', '--json', 'number,headRefName'], 65536, timeout=3)
    origins = {spec.upper()}
    pr_known = False
    try:
        entries = json.loads(prs)
        if not isinstance(entries, list):
            raise ValueError('invalid PR list')
        for entry in entries:
            if entry['headRefName'] == branch:
                origins.add('PR-' + str(int(entry['number'])))
        pr_known = len(entries) < 100 and bool(branch) and not branch.startswith('unknown (')
    except (ValueError, KeyError, TypeError):
        pass
    # Real stores need ~5 seconds; leave the remaining budget for persistence.
    rows = run(['aida', 'findings', 'list', '--json'], 65536, timeout=6.5)
    try:
        findings = json.dumps([row for row in json.loads(rows)['findings']
                               if row.get('origin', '').upper() in origins], ensure_ascii=False)
        encoded = findings.encode()
        findings = encoded[:2048].decode('utf-8', errors='ignore')
        if len(encoded) > 2048:
            findings += '\n[truncated]'
        if not pr_known:
            findings += '\nReview findings unknown (branch PR lookup unavailable).'
        if not spec_known:
            findings += '\nSpec findings unknown (branch pointer is not a resolved display ID).'
    except (ValueError, KeyError, TypeError):
        findings = 'unknown (findings unavailable or exceeded output cap)'
    # Always re-read the matched rotation note. Never recursively nest snapshots.
    prior = 'unknown (no session-bound prior handoff)'
    if os.environ.get('AIDA_ROTATION_HANDOFF'):
        prior = run(handoff + ['--show', '--body-only', '--rotated'], 2048)
    captured = datetime.now(timezone.utc).isoformat()
    body = (f'# PreCompact recovery snapshot (Claude; informational, revalidate before action)\n'
            f'Captured: {captured}\nSpec pointer: {spec}\nBranch: {branch}\n'
            f'Last verdict:\n{verdict}\nOpen findings (spec / branch PR origin):\n{findings}\n'
            f'Prior handoff / next-action context:\n{prior}\n'
            f'Next step: inspect `aida show {spec}` and the working diff; reconcile the '
            'verdict and findings, then continue the active spec. Unknown evidence stays unknown.\n')
    # Safely below session handoff’s 16 KiB cap even with multibyte diagnostics.
    body = body.encode()[:10000].decode('utf-8', errors='ignore')
    result = run(handoff + ['--snapshot', '--write', '-'], 1024, body)
    if result.startswith('unknown ('):
        print('AIDA PreCompact handoff unavailable: ' + result, file=sys.stderr)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError):
        # Optional recovery aid: failure must never block compaction or a lane.
        pass
