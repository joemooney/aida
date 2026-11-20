#!/usr/bin/env python3
"""Read-only advisor report of potential duplicate long flags across open GitHub PRs.

trace:STORY-2693 | ai:codex
"""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import subprocess
import sys


def added_flags(diff):
    """Recognize clap long declarations in Rust source, excluding removed names.

    Reports are deliberately advisory: the same flag may be legitimate in distinct
    subcommands. Never interpret a candidate as merge or completion authority.
    """
    added, removed = set(), set()
    source = False
    for line in diff.splitlines():
        if line.startswith('+++ '):
            source = line.startswith('+++ b/') and line.endswith('.rs')
        if not source or line[:1] not in ('+', '-') or line.startswith(('+++', '---')):
            continue
        target = added if line[0] == '+' else removed
        text = line[1:]
        for name in re.findall(r'\blong\s*=\s*"([a-zA-Z0-9][a-zA-Z0-9_-]*)"', text):
            target.add('--' + name)
        for name in re.findall(r'\.long\(\s*"([a-zA-Z0-9][a-zA-Z0-9_-]*)"\s*\)', text):
            target.add('--' + name)
    # Bare #[arg(long)] needs the following field; retain context lines for it.
    for sign, target in (('+', added), ('-', removed)):
        source = False
        pending = None
        for line in diff.splitlines():
            if line.startswith('+++ '):
                source = line.startswith('+++ b/') and line.endswith('.rs')
            if not source or line.startswith(('+++', '---')):
                continue
            if line.startswith(('@@', 'diff --git')):
                pending = None
            if not line.startswith((sign, ' ')):
                continue
            text = line[1:]
            if re.search(r'#\[(?:arg|clap)\([^\]]*\blong\s*(?:,|\))', text):
                pending = line.startswith(sign)
            field = re.search(r'^\s*(?:pub(?:\([^)]*\))?\s+)?([a-zA-Z_][a-zA-Z0-9_]*)\s*:', text)
            if pending is not None and field:
                if pending or line.startswith(sign):
                    target.add('--' + field[1].replace('_', '-'))
                pending = None
    return sorted(added - removed)


def report(prs):
    owners = defaultdict(set)
    for pr in prs:
        for flag in added_flags(pr['diff']):
            owners[flag].add(pr['number'])
    return [{'flag': flag, 'prs': sorted(ids)} for flag, ids in sorted(owners.items()) if len(ids) > 1]


def gh(*args):
    return subprocess.run(['gh', *args], check=True, text=True, capture_output=True, timeout=20).stdout


def live_prs():
    prs = json.loads(gh('pr', 'list', '--state', 'open', '--limit', '1000',
                        '--json', 'number,headRefOid,isDraft,headRefName'))
    if len(prs) >= 1000:
        raise ValueError('open PR inventory reached limit; refusing an incomplete report')
    results = []
    for pr in sorted(prs, key=lambda p: p['number']):
        if pr['headRefName'].startswith('train/'):
            continue  # train copies are not independent flag definitions
        number = str(pr['number'])
        diff = gh('pr', 'diff', number, '--color', 'never')
        current = json.loads(gh('pr', 'view', number, '--json', 'headRefOid,state'))
        if current['headRefOid'] != pr['headRefOid'] or current['state'] != 'OPEN':
            raise ValueError(f'PR #{number} changed during inspection; retry')
        results.append(dict(pr, diff=diff))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, help='JSON array of {number, diff}; no forge access')
    args = parser.parse_args()
    try:
        prs = json.loads(args.fixture.read_text()) if args.fixture else live_prs()
        collisions = report(prs)
        print(json.dumps({'audience': 'advisor', 'advisory': True, 'checked_prs': len(prs),
                          'potential_flag_collisions': collisions}, indent=2))
        return 1 if collisions else 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f'open-PR flag check unavailable: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
