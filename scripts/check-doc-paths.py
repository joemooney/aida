#!/usr/bin/env python3
"""Check literal repository docs paths in every tracked non-archive text file.

Rust test directories and inline test-only modules are skipped. Other output
contracts, fixtures and historical proposals use exact, counted exceptions.
The archive inventory supplies rename history, not required coverage rows.
"""
# trace:STORY-1651 TASK-3161 | ai:codex
import collections
import csv
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
EXCEPTIONS = ROOT / 'scripts/doc-path-exceptions.json'
PATTERN = re.compile(r'(?<![\w/])docs/(?:[\w.-]+/)*[\w.-]+\.(?:md|html|cast|svg|png|json|yaml|toml)(?![\w.-])')


# Share the Rust literal/comment mask and brace matching with the writer audit.
_spec = importlib.util.spec_from_file_location('store_writer_mask', ROOT / 'scripts/check-store-writers.py')
_rust = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_rust)

TEST_CFG = r'#\s*\[\s*cfg\s*\(\s*test\s*,?\s*\)\s*\]'
MACRO = r'\b\w+\s*!\s*(?:\w+\s*)?[({\[]'
ITEM_START = re.compile(rf'(?P<cfg>{TEST_CFG})|{MACRO}')
DELIMITER = re.compile(r'[(){}\[\]]')
CLOSING = {'(': ')', '{': '}', '[': ']'}
ATTRIBUTE = re.compile(r'\s*#\s*\[')
MODULE = re.compile(r'\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(?:r#)?[^\W\d]\w*\s*\{')


def token_tree_end(masked, start):
    closing = [CLOSING[masked[start]]]
    for match in DELIMITER.finditer(masked, start + 1):
        token = match[0]
        if token in CLOSING:
            closing.append(CLOSING[token])
        elif token != closing.pop():
            break
        elif not closing:
            return match.end()
    raise ValueError("unbalanced Rust token tree")


def production_rust(source, text, root):
    parts = Path(source).parts[:-1]
    if 'tests' in parts:
        test_dir = parts.index('tests')
        if ('src' in parts[:test_dir] or test_dir == 0
                or root.joinpath(*parts[:test_dir], 'Cargo.toml').is_file()):
            return ''

    # Leave malformed or unrecognized Rust visible to the path checker.
    try:
        masked = _rust.mask_rust(text)
        chunks, end, cursor = [], 0, 0
        while item := ITEM_START.search(masked, cursor):
            cursor = item.end()
            if item.lastgroup != 'cfg':
                # Macro token trees can quote module syntax as production data.
                cursor = token_tree_end(masked, cursor - 1)
                continue
            while attribute := ATTRIBUTE.match(masked, cursor):
                cursor = token_tree_end(masked, attribute.end() - 1)
            module = MODULE.match(masked, cursor)
            if module:
                chunks.append(text[end:item.start()])
                end = cursor = _rust.block_end(masked, module.end() - 1)
        chunks.append(text[end:])
        return '\n'.join(chunks)
    except ValueError:
        return text


def references(text):
    # Ignore HTTP URLs; literal repo-relative paths in prose/backticks count.
    text = re.sub(r'https?://[^\s<>`"\)]+', '', text)
    return collections.Counter(m[0] for m in PATTERN.finditer(text))


def violations(source, text, moved, exceptions, root=ROOT):
    if source.endswith('.rs'):
        text = production_rust(source, text, root)
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
