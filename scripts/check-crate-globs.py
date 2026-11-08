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
        elif Counter(expected) - Counter(json.loads(old.stdout)):
            raise ValueError('crate glob baseline may only shrink relative to base')
    print(f'Crate glob ratchet: {sum(expected.values())} remaining imports; zero additions')


if __name__ == '__main__':
    main()
