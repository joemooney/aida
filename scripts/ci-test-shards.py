#!/usr/bin/env python3
"""CI libtest lanes; keep the workspace's tests and doctests without changing harnesses.

CLI integration targets come from Cargo metadata, never a hand-maintained list.
Measured-time order (seeded from main timings), then target name, makes alternating
halves deterministic and spreads the long binaries. Unbudgeted targets follow
in name order. --compile is the same workspace union, without running tests.
trace:TASK-1949 | ai:codex
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tomllib

SHARDS = ('lib', 'cli-0', 'cli-1', 'rest')


def cli_halves(targets, budgets):
    def order(target):
        key = f"{target['name']}:tests/{Path(target['src_path']).name}"
        return (-budgets.get(key, 0), target['name'])
    ordered = sorted(targets, key=order)
    return (ordered[::2], ordered[1::2])


def commands(shard, packages, budgets):
    """The four disjoint selections cover cargo test --workspace's default set."""
    cli = next(p for p in packages if p['name'] == 'aida-cli')
    lib = next(p for p in packages if p['name'] == 'aida-cli-lib')
    cli_tests = [t for t in cli['targets'] if 'test' in t['kind'] and t['test']]
    base = ['cargo', 'test', '--no-fail-fast']
    if shard == 'lib':
        return [base + ['-p', 'aida-cli-lib', '--lib']]
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
    status = 0
    for shard in SHARDS if args.shard == 'all' else [args.shard]:
        for command in commands(shard, packages, budgets):
            if args.profile:
                command += ['--profile', args.profile]
            print('+ ' + ' '.join(command), flush=True)
            code = subprocess.call(command, cwd=root)
            if code:
                status = 1  # --no-fail-fast also spans separate cargo invocations.
    return status


if __name__ == '__main__':
    sys.exit(main())
