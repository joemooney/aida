#!/usr/bin/env python3
"""Ratchet gate -> command-layer item edges; see gate-dependency-ratchet.md."""
# trace:TASK-3785 | ai:codex
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LEDGER = 'scripts/gate-dependencies-legacy.json'
TARGETS = frozenset('pr_cmd session seat_clone queue_cmd auto_complete awaiting_you orchestrator pr_ship pr_train drain_lock drain_state store_sync merge_hold'.split())
spec = importlib.util.spec_from_file_location('exec_lexer', ROOT / 'scripts/check-exec-context.py')
lexer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lexer)


def edges(source):
    """Unique (target module, first item) paths, including import bindings.

    Whole-file alias unions deliberately overcount shadowed names rather than
    letting a nested import conceal a dependency. Module imports alone are not
    item edges; uses through those imports are. Tests are included.
    """
    ts = lexer.tokens(source)
    imports, excluded = [], set()
    for i, token in enumerate(ts):
        if token == 'use':
            end = ts.index(';', i)
            imports.extend((alias, normalize(path, import_binding=True))
                           for alias, path in lexer.use_paths(ts[i + 1:end]))
            excluded.update(range(i, end + 1))
    aliases = {name: {(name,)} for name in TARGETS | {'crate'}}
    for _ in range(len(imports) + 1):
        changed = False
        for alias, path in imports:
            for resolved in resolve(path, aliases):
                if resolved[:1] == ('crate',):
                    if resolved == ('crate', '*') or (len(resolved) > 1 and resolved[1] in TARGETS and '*' in resolved):
                        raise ValueError('gate command-layer glob import is forbidden')
                    values = aliases.setdefault(alias, set())
                    if resolved not in values:
                        values.add(resolved)
                        changed = True
        if not changed:
            break
    found = set()

    def record(path):
        for canonical in resolve(path, aliases):
            if canonical[:1] == ('crate',):
                canonical = canonical[1:]
            if len(canonical) >= 2 and canonical[0] in TARGETS:
                found.add('crate::' + '::'.join(canonical[:2]))

    for _, path in imports:
        record(path)
    for i, token in enumerate(ts):
        if i in excluded or not lexer.IDENT.fullmatch(token):
            continue
        if i >= 2 and ts[i - 1] == '::' and lexer.IDENT.fullmatch(ts[i - 2]) and ts[i - 2] not in lexer.RUST_KEYWORDS:
            continue
        path, end = [token], i + 1
        while end + 1 < len(ts) and ts[end] == '::' and lexer.IDENT.fullmatch(ts[end + 1]):
            path.append(ts[end + 1])
            end += 2
        record(normalize(tuple(path)))
    return found


def normalize(path, *, import_binding=False):
    # Relative ancestor paths into a watched root module are also upward edges.
    original = path
    while path and path[0] in {'self', 'super'}:
        path = path[1:]
    # trace:TASK-3785 | ai:codex
    # Without source-module context an ancestor-only path has no canonical
    # identity. Dropping it would lose aliases (also grouped `self as name`)
    # and hide every subsequent item reference through that binding.
    if import_binding and not path and 'super' in original:
        raise ValueError('unresolved ancestor-module path ' + '::'.join(original)
                         + '; use an explicit crate:: path')
    if path and path[0] in TARGETS:
        path = ('crate',) + path
    return path


def resolve(path, aliases):
    return {prefix + path[1:] for prefix in aliases.get(path[0], {path[:1]})} if path else set()


def scan(root=ROOT):
    # Include new files before staging, as native lanes cannot stage Git metadata.
    directory = root / 'aida-cli-lib/src/gate'
    paths = sorted(directory.rglob('*.rs'))
    if not paths:
        raise ValueError('gate source inventory is empty')
    return {str(path.relative_to(root)): sorted(found) for path in paths
            if (found := edges(path.read_text(encoding='utf-8')))}


def validate(ledger):
    if ledger['version'] != 1 or ledger['owner'] != 'TASK-3785':
        raise ValueError('invalid gate ledger header')
    sites = ledger['edges']
    if any(not isinstance(items, list) or items != sorted(set(items)) for items in sites.values()):
        raise ValueError('gate ledger edges must be sorted and unique')
    if type(ledger['ceiling']) is not int or ledger['ceiling'] != sum(map(len, sites.values())):
        raise ValueError('invalid gate ledger ceiling')


def check_growth(ledger, previous):
    validate(ledger)
    validate(previous)
    if ledger['ceiling'] > previous['ceiling']:
        raise ValueError('gate dependency ceiling may only decrease relative to base')
    # A retirement cannot pay for a different upward dependency. File relocation
    # within gate is allowed, but new target items are not.
    current = {edge for items in ledger['edges'].values() for edge in items}
    old = {edge for items in previous['edges'].values() for edge in items}
    if current - old:
        raise ValueError(f'new gate dependency items relative to base: {sorted(current - old)}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', help='refuse ceiling growth relative to this Git revision')
    parser.add_argument('--write-ledger', action='store_true', help='capture/prune current edges')
    args = parser.parse_args()
    actual = scan()
    ledger = ({'version': 1, 'owner': 'TASK-3785', 'ceiling': sum(map(len, actual.values())), 'edges': actual}
              if args.write_ledger else json.loads((ROOT / LEDGER).read_text()))
    validate(ledger)
    if ledger['edges'] != actual:
        raise ValueError('gate dependency ledger mismatch; inspect changes and prune with --write-ledger --base REF')
    if args.base:
        base = subprocess.check_output(['git', 'rev-parse', '--verify', args.base + '^{commit}'], cwd=ROOT, text=True).strip()
        files = subprocess.check_output(['git', 'ls-tree', '--name-only', base, LEDGER], cwd=ROOT, text=True).splitlines()
        if LEDGER in files:
            previous = json.loads(subprocess.check_output(['git', 'show', f'{base}:{LEDGER}'], cwd=ROOT, text=True))
            check_growth(ledger, previous)
    if args.write_ledger:
        (ROOT / LEDGER).write_text(json.dumps(ledger, indent=2, sort_keys=True) + '\n')
    print(f"Gate dependency ratchet: {ledger['ceiling']} upward item edges; zero additions")


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
