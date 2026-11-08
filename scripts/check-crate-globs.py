#!/usr/bin/env python3
"""Forbid growth of root crate glob imports; prune the baseline as imports migrate."""
# trace:TASK-3428 | ai:codex
import argparse
from collections import Counter
import importlib.util
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LEDGER = 'scripts/crate-globs-legacy.json'
spec = importlib.util.spec_from_file_location('exec_ratchet', ROOT / 'scripts/check-exec-context.py')
lexer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lexer)


def count(source):
    ts = lexer.tokens(source)
    result = 0
    for i, token in enumerate(ts):
        if token == 'use':
            end = ts.index(';', i)
            result += sum(path == ('crate', '*') for _, path in lexer.use_paths(ts[i + 1:end]))
    return result


def scan():
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--', '*.rs'], cwd=ROOT).decode().split('\0')
    return {p: n for p in paths if p and (ROOT / p).exists() and (n := count((ROOT / p).read_text()))}


def check(actual, expected):
    if actual != expected:
        raise ValueError(f'crate glob baseline mismatch: new={dict(Counter(actual) - Counter(expected))}; prune={dict(Counter(expected) - Counter(actual))}')


def check_growth(expected, previous, base):
    # trace:TASK-3428 | ai:codex
    # A retired import budget transfers only through an actual Git rename.
    changes = subprocess.check_output(
        ['git', 'diff', '--name-status', '-z', '-M', base, '--', '*.rs'], cwd=ROOT
    ).decode().split('\0')
    renamed = {}
    i = 0
    while i < len(changes) and changes[i]:
        status = changes[i]
        i += 1
        if status.startswith(('R', 'C')):
            old, new = changes[i:i + 2]
            i += 2
            if status.startswith('R'):
                renamed[new] = old
        else:
            i += 1
    for path, count in expected.items():
        if path in previous:
            allowed = count <= previous[path]
        else:
            old = renamed.get(path)
            allowed = old in previous and old not in expected and count == previous[old]
        if not allowed:
            raise ValueError('crate glob baseline may only shrink or follow same-count Git renames relative to base')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base')
    args = parser.parse_args()
    expected = json.loads((ROOT / LEDGER).read_text())
    check(scan(), expected)
    if args.base:
        old = subprocess.run(['git', 'show', f'{args.base}:{LEDGER}'], cwd=ROOT, capture_output=True, text=True)
        if old.returncode:
            # Only bootstrap when the base commit exists and has no ledger.
            subprocess.run(['git', 'rev-parse', '--verify', args.base + '^{commit}'], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
            files = subprocess.check_output(['git', 'ls-tree', '--name-only', args.base, LEDGER], cwd=ROOT)
            if files.strip():
                raise ValueError('cannot read base glob ledger')
        else:
            check_growth(expected, json.loads(old.stdout), args.base)
    print(f'Crate glob ratchet: {sum(expected.values())} remaining imports; zero additions')


if __name__ == '__main__':
    main()
