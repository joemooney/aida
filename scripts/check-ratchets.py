#!/usr/bin/env python3
"""Run the checked application ratchets alongside the shared source guards.

trace:STORY-2833 | ai:codex
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]

# trace:BUG-2664 | ai:codex
CARGO_DIFF_SKIP = re.compile(
    r'cargo: no compile inputs changed; CI covers it \(.+\)\. '
    r'Skipped; pass --force-build to run anyway\.'
)


# A change under any of these can move what docs/cli/verify-manual.py checks.
# trace:BUG-2782 | ai:claude
MANUAL_SURFACE = re.compile(
    r'^(docs/cli/|aida-cli/|aida-cli-lib/src/(.+_args\.rs|command_registry\.rs|cli\.rs'
    r'|tests/golden/cli_help_snapshot\.txt)$|scripts/check-ratchets\.py$)'
)


def changed_paths(base='origin/main'):
    """Paths changed vs the merge base: committed, staged, unstaged, untracked.

    None when git cannot answer; the caller then runs the manual check.
    """
    commands = (['git', '-c', 'core.quotePath=false', 'diff', '--name-only', f'{base}...HEAD'],
                ['git', '-c', 'core.quotePath=false', 'diff', '--name-only', 'HEAD'],
                ['git', '-c', 'core.quotePath=false', 'ls-files', '--others', '--exclude-standard'])
    paths = set()
    for command in commands:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        if result.returncode:
            return None
        paths.update(line for line in result.stdout.splitlines() if line)
    return paths


def manual_reason(mode, paths):
    """Why the manual check runs, or None to skip it."""
    if mode == 'always':
        return 'always (--manual always)'
    if paths is None:
        return 'cannot diff against origin/main, so running it'
    touched = sorted(path for path in paths if MANUAL_SURFACE.match(path))
    if touched:
        more = f' (+{len(touched) - 1} more)' if len(touched) > 1 else ''
        return f'CLI surface changed: {touched[0]}{more}'
    return None


def run(command, env=None, require_tests=False, expected_tests=None, why=None):
    if why:
        print(f'ratchets: step: {why}', flush=True)
    print('+ ' + ' '.join(command), flush=True)
    start = time.monotonic()
    # Stream compiler output while tracking test results and the wrapper's diff skip.
    child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True)
    passed = 0
    saw_test_result = False
    cargo_diff_skip = False
    for line in child.stdout:
        print(line, end='', flush=True)
        match = re.search(r'test result: ok\. (\d+) passed;', line)
        if match:
            saw_test_result = True
            passed += int(match[1])
        if CARGO_DIFF_SKIP.fullmatch(line.rstrip('\n')):
            cargo_diff_skip = True
    child.stdout.close()
    code = child.wait()
    if code == 0 and command[:2] == ['cargo', 'test'] and cargo_diff_skip and not saw_test_result:
        print(f'ratchets: skipped {" ".join(command)} (cargo wrapper diff skip)', flush=True)
        return
    print(f'ratchets: {time.monotonic() - start:.2f}s exit={code} passed={passed}', flush=True)
    if code:
        raise SystemExit(code)
    if require_tests and not passed:
        raise SystemExit('ratchets: refusing a successful command with no executed tests')
    if expected_tests is not None and passed != expected_tests:
        raise SystemExit(f'ratchets: expected {expected_tests} executed tests, got {passed}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', default='agent', choices=['agent', 'dev'])
    parser.add_argument('--manual', default='always', choices=['always', 'auto'],
                        help='auto: build aida and run verify-manual only when the manual '
                             'or CLI surface changed vs origin/main (CI uses always)')
    args = parser.parse_args()
    manifest = json.loads((ROOT / 'scripts/ratchet-set.json').read_text())
    run([sys.executable, '-m', 'unittest', 'tests/test_ratchet_set.py',
         'tests/test_ci_structural_guards.py', 'tests/test_store_writer_inventory.py',
         'tests/test_verify_manual.py'], why='ratchet-set unit tests (no build)')
    run([sys.executable, 'scripts/check-store-writers.py'], why='store-writer inventory (no build)')
    run(['cargo', 'test', '--profile', args.profile, '-p', 'aida-ci-guards', '--lib'],
        require_tests=True, expected_tests=sum(map(len, manifest['shared_modules'].values())),
        why=f'builds and runs aida-ci-guards lib tests ({args.profile}): shared source guards')
    for group in manifest['groups']:
        run(['cargo', 'test', '--profile', args.profile, '-p', group['package'],
             *group['target'], '--', group['filter']], require_tests=True,
            expected_tests=len(group['tests']),
            why=f'builds and runs {group["package"]} {" ".join(group["target"])} tests '
                f'({args.profile}): ratchet group {group["filter"]}')
    reason = manual_reason(args.manual, changed_paths() if args.manual == 'auto' else set())
    if reason is None:
        print('ratchets: skipped aida build and docs/cli/verify-manual.py: no docs/cli or '
              'CLI-surface change vs origin/main (CI always runs it)', flush=True)
        return
    # Dynamic manual checks must inspect this tree's freshly built executable.
    run(['cargo', 'build', '--profile', args.profile, '-p', 'aida-cli', '--bin', 'aida'],
        why=f'builds aida ({args.profile}) for docs/cli/verify-manual.py: {reason}')
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    env = dict(os.environ, AIDA_BIN=str(target / ('debug' if args.profile == 'dev' else args.profile) / 'aida'))
    run([sys.executable, 'docs/cli/verify-manual.py'], env=env, why=f'CLI-manual drift guard: {reason}')


if __name__ == '__main__':
    main()
