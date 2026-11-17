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


def run(command, env=None, require_tests=False, expected_tests=None):
    print('+ ' + ' '.join(command), flush=True)
    start = time.monotonic()
    # Stream compiler output; keep only test-result lines to reject empty filters.
    child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True)
    passed = 0
    for line in child.stdout:
        print(line, end='', flush=True)
        match = re.search(r'test result: ok\. (\d+) passed;', line)
        if match:
            passed += int(match[1])
    child.stdout.close()
    code = child.wait()
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
    args = parser.parse_args()
    manifest = json.loads((ROOT / 'scripts/ratchet-set.json').read_text())
    run([sys.executable, '-m', 'unittest', 'tests/test_ratchet_set.py',
         'tests/test_ci_structural_guards.py', 'tests/test_store_writer_inventory.py',
         'tests/test_verify_manual.py'])
    run([sys.executable, 'scripts/check-store-writers.py'])
    run(['cargo', 'test', '--profile', args.profile, '-p', 'aida-ci-guards', '--lib'],
        require_tests=True, expected_tests=sum(map(len, manifest['shared_modules'].values())))
    for group in manifest['groups']:
        run(['cargo', 'test', '--profile', args.profile, '-p', group['package'],
             *group['target'], '--', group['filter']], require_tests=True,
            expected_tests=len(group['tests']))
    # Dynamic manual checks must inspect this tree's freshly built executable.
    run(['cargo', 'build', '--profile', args.profile, '-p', 'aida-cli', '--bin', 'aida'])
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    env = dict(os.environ, AIDA_BIN=str(target / ('debug' if args.profile == 'dev' else args.profile) / 'aida'))
    run([sys.executable, 'docs/cli/verify-manual.py'], env=env)


if __name__ == '__main__':
    main()
