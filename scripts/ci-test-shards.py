#!/usr/bin/env python3
"""CI libtest lanes; keep the workspace's tests and doctests without changing harnesses.

CLI integration targets come from Cargo metadata, never a hand-maintained list.
Measured-time order (seeded from main timings), then target name, makes alternating
halves deterministic and spreads the long binaries. Unbudgeted targets follow
in name order. --compile is the same workspace union, without running tests.
trace:TASK-1949 | ai:codex

The aida-cli-lib unit binary is one target, so its halves split it by test
name: lib-N lists the binary's tests and runs its half of the top-level modules
as libtest filters. Filters are substrings, so modules whose `<module>::` filter
also matches another module's tests share a half; the halves are disjoint and
cover every listed test, or the lane fails. Before running anything, every lane
checks that the shards' union is cargo test --workspace's target set.
trace:TASK-1979 | ai:claude
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib

SHARDS = ('lib-0', 'lib-1', 'cli-0', 'cli-1', 'rest')
LIB_UNIT = ['-p', 'aida-cli-lib', '--lib']


def cli_halves(targets, budgets):
    def order(target):
        key = f"{target['name']}:tests/{Path(target['src_path']).name}"
        return (-budgets.get(key, 0), target['name'])
    ordered = sorted(targets, key=order)
    return (ordered[::2], ordered[1::2])


def module_of(name):
    return name.split('::', 1)[0]


def lib_filter(module, names):
    # A root-level test has no module; its own name is its filter.
    return f'{module}::' if any(n.startswith(f'{module}::') for n in names) else module


def matched(filters, names):
    """libtest's default filter semantics: a test runs if any filter is a substring."""
    return {n for n in names if any(f in n for f in filters)}


def lib_halves(names):
    """Two disjoint filter lists over the lib's test names, balanced by test count.

    trace:TASK-1979 | ai:claude
    """
    names = sorted(set(names))
    modules = sorted({module_of(n) for n in names})
    parent = {m: m for m in modules}

    def root(m):
        while parent[m] != m:
            m = parent[m]
        return m
    filters = {m: lib_filter(m, names) for m in modules}
    for m, f in filters.items():
        for n in names:
            if f in n:
                a, b = sorted((root(m), root(module_of(n))))
                parent[b] = a
    groups = {}
    for n in names:
        groups.setdefault(root(module_of(n)), set()).add(module_of(n))
    weight = {g: sum(1 for n in names if root(module_of(n)) == g) for g in groups}
    halves, counts = ([], []), [0, 0]
    for g in sorted(groups, key=lambda g: (-weight[g], g)):
        side = 0 if counts[0] <= counts[1] else 1
        halves[side].extend(filters[m] for m in sorted(groups[g]))
        counts[side] += weight[g]
    runs = [matched(h, names) for h in halves]
    if runs[0] & runs[1] or runs[0] | runs[1] != set(names) or not all(halves):
        raise ValueError('lib halves are not a disjoint cover of the listed tests')
    return tuple(sorted(h) for h in halves)


def commands(shard, packages, budgets):
    """The four disjoint selections cover cargo test --workspace's default set."""
    cli = next(p for p in packages if p['name'] == 'aida-cli')
    lib = next(p for p in packages if p['name'] == 'aida-cli-lib')
    cli_tests = [t for t in cli['targets'] if 'test' in t['kind'] and t['test']]
    base = ['cargo', 'test', '--no-fail-fast']
    if shard.startswith('lib-'):
        return [base + LIB_UNIT]  # main() appends this half's filters.
    if shard.startswith('cli-'):
        targets = cli_halves(cli_tests, budgets)[int(shard[-1])]
        if not targets:
            raise ValueError('empty CLI test shard')
        return [base + ['-p', 'aida-cli'] + [arg for t in targets for arg in ('--test', t['name'])]]
    if shard == 'rest':
        result = [base + ['--workspace', '--exclude', 'aida-cli', '--exclude', 'aida-cli-lib',
                          '--features', 'aida-core/postgres,aida-core/gitlab,aida-core/github,aida-core/jira'],
                  base + ['-p', 'aida-cli', '--bins'],
                  base + ['-p', 'aida-cli-lib', '--doc']]
        extra = [t for t in lib['targets'] if 'test' in t['kind'] and t['test']]
        if extra:
            result.append(base + ['-p', 'aida-cli-lib'] + [arg for t in extra for arg in ('--test', t['name'])])
        return result
    raise ValueError(f'unknown shard: {shard}')


def units(package):
    """What `cargo test` runs for a package by default: every test-enabled target, plus doctests."""
    found = {(package['name'], 'bin' if 'bin' in t['kind'] else 'lib' if 'lib' in t['kind'] else t['kind'][0],
              t['name']) for t in package['targets'] if t.get('test') and 'custom-build' not in t['kind']}
    found |= {(package['name'], 'doc', t['name']) for t in package['targets'] if t.get('doctest')}
    return found


def selected(command, packages):
    """The (package, kind, target) units one planned `cargo test` command selects."""
    args = command[2:]
    if '--workspace' in args:
        excluded = {args[i + 1] for i, a in enumerate(args) if a == '--exclude'}
        chosen = [p for p in packages if p['name'] not in excluded]
    else:
        names = {args[i + 1] for i, a in enumerate(args) if a == '-p'}
        chosen = [p for p in packages if p['name'] in names]
    tests = {args[i + 1] for i, a in enumerate(args) if a == '--test'}
    kinds = {k for flag, k in (('--lib', 'lib'), ('--bins', 'bin'), ('--doc', 'doc')) if flag in args}
    every = set().union(*map(units, chosen))
    if not tests and not kinds:
        return every
    return {u for u in every if u[1] in kinds or (u[1] == 'test' and u[2] in tests)}


def union_gaps(packages, budgets):
    """Targets cargo test --workspace runs that no shard selects.

    trace:TASK-1979 | ai:claude
    """
    covered = set()
    for shard in SHARDS:
        for command in commands(shard, packages, budgets):
            covered |= selected(command, packages)
    return sorted(set().union(*map(units, packages)) - covered)


def lib_test_names(root, profile):
    command = ['cargo', 'test'] + LIB_UNIT + profile + ['--', '--list', '--format', 'terse']
    print('+ ' + ' '.join(command), flush=True)
    # The listing's own "Running" line would read as an unfinished binary in the timings log.
    listing = subprocess.run(command, cwd=root, capture_output=True, text=True)
    if listing.returncode:
        sys.stderr.write(listing.stderr)
        raise ValueError(f'listing the lib tests exited {listing.returncode}')
    return [line[:-len(': test')] for line in listing.stdout.splitlines() if line.endswith(': test')]


def check_root(root):
    # --directory also catches a new directory at root, not just loose files.
    raw = subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard',
                                   '--directory', '-z'], cwd=root)
    paths = [p.decode(errors='replace') for p in raw.split(b'\0') if p]
    leaks = [p for p in paths if '/' not in p.rstrip('/')]
    if leaks:
        print('::error::test shard left untracked root entries: ' + ', '.join(leaks), file=sys.stderr)
        return 1
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--shard', choices=(*SHARDS, 'all'))
    mode.add_argument('--compile', action='store_true')
    mode.add_argument('--check-root', action='store_true')
    parser.add_argument('--profile')
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parents[1]
    if args.check_root:
        return check_root(root)
    if args.compile:
        return subprocess.call(['cargo', 'test', '--workspace', '--no-run'] +
                               (['--profile', args.profile] if args.profile else []), cwd=root)
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=root))
    packages = [p for p in metadata['packages'] if p['id'] in metadata['workspace_members']]
    with (root / 'scripts/test-time-budget.toml').open('rb') as handle:
        budgets = tomllib.load(handle)['shard_seconds']
    gaps = union_gaps(packages, budgets)
    if gaps:
        print('::error::no test shard runs these cargo test --workspace targets: ' +
              ', '.join(':'.join(gap) for gap in gaps), file=sys.stderr)
        return 1
    profile = ['--profile', args.profile] if args.profile else []
    status = 0
    for shard in SHARDS if args.shard == 'all' else [args.shard]:
        if shard == 'lib-1' and args.shard == 'all':
            continue  # lib-0 ran the whole binary.
        for command in commands(shard, packages, budgets):
            command += profile
            if shard.startswith('lib-') and args.shard != 'all':
                try:
                    filters = lib_halves(lib_test_names(root, profile))[int(shard[-1])]
                except ValueError as err:
                    print(f'::error::cannot plan {shard}: {err}', file=sys.stderr)
                    return 1
                print(f'# {shard}: {len(filters)} module filters', flush=True)
                command += ['--'] + filters
            print('+ ' + ' '.join(command), flush=True)
            code = subprocess.call(command, cwd=root)
            if code:
                status = 1  # --no-fail-fast also spans separate cargo invocations.
    return status


if __name__ == '__main__':
    sys.exit(main())
