#!/usr/bin/env python3
"""Check literal repository docs paths in every tracked non-archive text file.

Output contracts, test fixtures and historical proposals use exact, counted
exceptions in doc-path-exceptions.json; adding a reference needs a decision.
The archive inventory supplies rename history, not required coverage rows.
"""
# trace:STORY-1651 | ai:codex
import collections
import csv
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
EXCEPTIONS = ROOT / 'scripts/doc-path-exceptions.json'
PATTERN = re.compile(r'(?<![\w/])docs/(?:[\w.-]+/)*[\w.-]+\.(?:md|html|cast|svg|png|json|yaml|toml)(?![\w.-])')


def references(text):
    # Ignore HTTP URLs; literal repo-relative paths in prose/backticks count.
    text = re.sub(r'https?://[^\s<>`"\)]+', '', text)
    return collections.Counter(m[0] for m in PATTERN.finditer(text))


def violations(source, text, moved, exceptions, root=ROOT):
    errors = []
    for target, count in references(text).items():
        allowed = exceptions.get((source, target), 0)
        if count <= allowed:
            continue
        if target in moved or not (root / target).exists():
            reason = f'moved to {moved[target]}' if target in moved else 'missing'
            errors.append(f'{source}: {target}: {reason} ({count - allowed} unapproved reference(s))')
    return errors


def check():
    with (ROOT / 'docs/archive/2026-10-cleanup/inventory.tsv').open() as f:
        moved = {r['path_before_cleanup']: r['path'] for r in csv.DictReader(f, delimiter='\t')
                 if r['path_before_cleanup'] != r['path']}
    entries = json.loads(EXCEPTIONS.read_text())
    exceptions = {(r['source'], r['target']): r['count'] for r in entries}
    args = ['git', 'ls-files', '-z']
    if '--working-tree' in sys.argv:
        args += ['--cached', '--others', '--exclude-standard']
    paths = set(subprocess.check_output(args, cwd=ROOT).decode().split('\0'))
    errors = []
    for source in sorted(paths):
        if not source or source.startswith('docs/archive/') or source == EXCEPTIONS.relative_to(ROOT).as_posix():
            continue
        path = ROOT / source
        if not path.is_file():
            continue
        try:
            content = path.read_text()
        except UnicodeError:
            continue
        errors += violations(source, content, moved, exceptions)
    for error in errors:
        print(error)
    print(f'Tracked documentation paths: {len(errors)} invalid references')
    return int(bool(errors))


if __name__ == '__main__':
    sys.exit(check())
