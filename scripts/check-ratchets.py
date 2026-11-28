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
from collections import Counter
import importlib.util
import tomllib

ROOT = Path(__file__).resolve().parents[1]
_mask_spec = importlib.util.spec_from_file_location('store_writer_mask', ROOT / 'scripts/check-store-writers.py')
_mask = importlib.util.module_from_spec(_mask_spec)
_mask_spec.loader.exec_module(_mask)


# --only-set: emulate cargo's test selection from source, with no build, so a
# test added to or removed from a ratchet module fails before push instead of
# as "expected N executed tests, got M" after CI compiles. trace:TASK-3937 | ai:claude
SOURCE_TOKEN = re.compile(r'#!?\[|[{};]|\bmod\s+(?:r#)?(\w+)\s*([;{])|\bfn\s+(?:r#)?(\w+)|\binclude!\s*\(()')
TEST_ATTR = re.compile(r'#\[\s*(?:\w+::)*test\b')
CFG_ATTR = re.compile(r'^#\[\s*cfg\s*\((.*)\)\s*\]$', re.S)
PATH_ATTR = re.compile(r'^#\[\s*path\s*=\s*"([^"]+)"\s*\]$')


def _cfg(expr, features):
    """A cfg predicate on CI's Linux runners; an unknown key counts as enabled."""
    def atom(m):
        if m[3]:
            return {'all': 'all((', 'any': 'any((', 'not': 'not_(('}[m[3]]
        if m[1]:
            return str(m[2] in features if m[1] == 'feature' else
                       m[1] not in ('target_os', 'target_family') or m[2] in ('linux', 'unix'))
        return str(m[0] not in ('windows', 'miri'))
    python = re.sub(r'(\w+)\s*=\s*"([^"]*)"|\b(all|any|not)\s*\(|\w+', atom, expr)
    python = re.sub(r'\(\(,', '((', re.sub(r'\)', ',))', python))
    return eval(python, {'__builtins__': {}, 'all': all, 'any': any, 'not_': lambda v: not v[0]})


def default_features(package_dir):
    table = tomllib.loads((package_dir / 'Cargo.toml').read_text()).get('features', {})
    seen, todo = set(), ['default']
    while todo:
        if (name := todo.pop()) not in seen and '/' not in name and not name.startswith('dep:'):
            seen.add(name)
            todo.extend(table.get(name, []))
    return seen


def crate_tests(root_file, features):
    """[(file, 'mod::path::fn')] for every non-ignored test of the crate rooted at root_file."""
    found = []

    def walk(path, prefix, child_dir):
        source = path.read_text()
        masked = _mask.mask_rust(source)
        stack, attrs, pos = [], [], 0  # stack: inline module name, or None for any other block
        while match := SOURCE_TOKEN.search(masked, pos):
            token, pos = match.group(), match.end()
            if token.endswith('['):
                depth = 0
                for pos in range(match.end() - 1, len(masked)):
                    depth += {'[': 1, ']': -1}.get(masked[pos], 0)
                    if not depth:
                        break
                pos += 1
                attrs += [re.sub(r'\s+', ' ', source[match.start():pos])] if token == '#[' else []
                continue
            enabled = all(_cfg(cfg[1], features) for a in attrs if (cfg := CFG_ATTR.match(a)))
            current, attrs, inline = attrs, [], None not in stack
            if token in '{}':
                stack.append(None) if token == '{' else stack and stack.pop()
            elif match[1] and (not enabled or not inline) and match[2] == '{':
                pos = _mask.block_end(masked, match.end() - 1)  # a disabled module, or one in a fn
            elif match[1] and enabled and inline and match[2] == '{':
                stack.append(match[1])
            elif match[1] and enabled and inline:
                base = child_dir.joinpath(*stack)
                explicit = next((p[1] for a in current if (p := PATH_ATTR.match(a))), None)
                if explicit:
                    child = (path.parent if not stack else base) / explicit
                else:
                    child = base / f'{match[1]}.rs'
                    child = child if child.is_file() else base / match[1] / 'mod.rs'
                grand = child.parent if child.name == 'mod.rs' or explicit else child.parent / child.stem
                if child.is_file():
                    walk(child, prefix + stack + [match[1]], grand)
            elif match[4] is not None and enabled and inline:  # include!("file") splices in place
                if literal := re.match(r'\s*"([^"\\]*)"', source[match.start(4):]):
                    walk(path.parent / literal[1], prefix + stack, child_dir.joinpath(*stack))
            elif match[3] and enabled and inline and any(TEST_ATTR.match(a) for a in current) \
                    and not any(a.startswith(('#[ignore]', '#[ignore ', '#[ignore=')) for a in current):
                found.append((path.resolve(), '::'.join(prefix + stack + [match[3]])))

    walk(root_file, [], root_file.parent)
    return found


def set_drift(manifest, root=ROOT):
    """Each test cargo would run that the manifest omits, and each it lists that cargo would not run."""
    root, problems, cache = Path(root).resolve(), [], {}

    def tests_of(package, target):
        package_dir = root / package
        if target == ['--lib']:
            crate = package_dir / tomllib.loads((package_dir / 'Cargo.toml').read_text()).get('lib', {}).get(
                'path', 'src/lib.rs')
        else:  # ['--test', name]
            crate = package_dir / 'tests' / f'{target[1]}.rs'
        if crate not in cache:
            cache[crate] = crate_tests(crate, default_features(package_dir))
        return cache[crate]

    def compare(label, listed, run):
        expected = Counter(((root / file).resolve(), name) for file, name in listed)
        shown = {(file, test.rsplit('::', 1)[-1]): test for file, test in run}
        actual = Counter((file, test.rsplit('::', 1)[-1]) for file, test in run)
        problems.extend(f'{label}: {os.path.relpath(file, root)}: test {shown[file, name]} runs under cargo '
                        f'but is not listed in scripts/ratchet-set.json' + (f' ({n}x)' if n > 1 else '')
                        for (file, name), n in (actual - expected).items())
        problems.extend(f'{label}: {os.path.relpath(file, root)}: listed test {name} is not a test cargo '
                        'runs (removed, renamed or ignored?)' for file, name in expected - actual)

    # Source cannot count macro-generated tests; refuse them in a ratchet module rather than miscount.
    for file in sorted({g['source'] for g in manifest['groups']} | set(manifest['shared_modules'])):
        masked = _mask.mask_rust((root / file).read_text())
        for macro in re.finditer(r'\bmacro_rules!\s*\w+\s*\{|\bproptest!', masked):
            body = masked[macro.start():_mask.block_end(masked, macro.end() - 1)] \
                if macro.group().endswith('{') else '#[test]'
            if TEST_ATTR.search(body):
                line = masked.count('\n', 0, macro.start()) + 1
                problems.append(f'{file}: a macro-generated test (line {line})'
                                ' cannot be checked from source; write ratchet tests as plain #[test] fns')
    compare('shared_modules (aida-ci-guards --lib)',
            [(file, name) for file, names in manifest['shared_modules'].items() for name in names],
            tests_of('aida-ci-guards', ['--lib']))
    for group in manifest['groups']:
        compare(f'group {group["filter"]} ({group["package"]} {" ".join(group["target"])})',
                [(group['source'], name) for name in group['tests']],
                [(file, test) for file, test in tests_of(group['package'], group['target'])
                 if group['filter'] in test])
    return problems


def only_set(manifest):
    for problem in (problems := set_drift(manifest)):
        print(f'ratchets: {problem}', flush=True)
    if problems:
        raise SystemExit(f'ratchets: --only-set: {len(problems)} drift(s); update scripts/ratchet-set.json')
    total = sum(map(len, manifest['shared_modules'].values())) + sum(len(g['tests']) for g in manifest['groups'])
    print(f'ratchets: --only-set: the manifest matches the {total} tests cargo would run (no build)', flush=True)


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
    parser.add_argument('--only-set', action='store_true',
                        help='no cargo: check from source that scripts/ratchet-set.json lists exactly '
                             'the tests each ratchet cargo run would execute, then stop')
    args = parser.parse_args()
    manifest = json.loads((ROOT / 'scripts/ratchet-set.json').read_text())
    if args.only_set:
        return only_set(manifest)
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
