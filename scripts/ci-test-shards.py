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

--runner nextest runs the same selections under cargo-nextest's `ci` profile
(.config/nextest.toml) for the non-gating shadow lanes; doctests stay on
cargo test. Each nextest invocation's JUnit report is kept as
junit-<shard>-<n>.xml. --totals sums passed/failed/ignored from either
runner's log, so a shadow lane is compared with its cargo-test lane by count.
trace:TASK-1957 | ai:claude
"""
from __future__ import annotations

import argparse
import collections
import datetime
import importlib.util
import re
import json
from pathlib import Path
import subprocess
import sys
import tomllib

SHARDS = ('lib-0', 'lib-1', 'cli-0', 'cli-1', 'rest')
_spec = importlib.util.spec_from_file_location('test_touched', Path(__file__).with_name('test-touched.py'))
_touched = sys.modules.setdefault('test_touched', importlib.util.module_from_spec(_spec))
_spec.loader.exec_module(_touched)
LIB_KINDS = set(_touched.LIB_KINDS)  # One definition of a library target's kinds.
LIB_UNIT = ['-p', 'aida-cli-lib', '--lib']
NEXTEST_PROFILE = 'ci'
ANSI_RE = re.compile(r'\x1b\[[0-9;]*[A-Za-z]')
TARGET_FILTERS = {'--lib', '--bins', '--bin', '--test', '--tests', '--doc'}
LIBTEST_HEADER_RE = re.compile(r'^\s*(?:Running \S|Doc-tests \S)')
LIBTEST_RE = re.compile(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;')
# `Summary [  12.3s] 120 tests run: 118 passed, 2 failed, 1 exec failed, 3 skipped`;
# a cancelled run prints `118/120 tests run`. skipped is #[ignore] plus every
# test a name filter excluded; FILTERED_RE's marker says how many of those.
NEXTEST_RE = re.compile(r'Summary \[\s*[\d.]+s\] \d+(?:/\d+)? tests? run: (.*)$')
NEXTEST_PART_RE = re.compile(r'(\d+) (passed|failed|exec failed|timed out|skipped)')
FILTERED_RE = re.compile(r'^# \S+: \d+ module filters, (\d+) tests? filtered out$')
# nextest's own output: its start line and the headers of captured test output
# (0.9.146: `  stdout ───`, or `── stdout ──` without indentation).
NEXTEST_OUTPUT_RE = re.compile(r'^\s*(?:Starting \d+ tests?|(?:─+ )?(?:stdout|stderr|output|execfail) ─+$)')


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


def lib_halves(names, seconds=None):
    """Two disjoint filter lists over the lib's test names, balanced by time.

    `seconds` maps a top-level module to its measured test seconds (summed
    per-test, from CI logs; see --lib-weights). A module without an entry
    weighs its test count times the measured mean per test. Ties fall to
    test count, then module name, so the split is deterministic.
    trace:TASK-1979 | ai:claude
    """
    seconds = seconds or {}
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
    count = collections.Counter(module_of(n) for n in names)
    known = [m for m in modules if m in seconds]
    mean = sum(seconds[m] for m in known) / max(1, sum(count[m] for m in known)) if known else 1.0
    cost = {m: seconds.get(m, count[m] * mean) for m in modules}
    weight = {g: (sum(cost[m] for m in ms), sum(count[m] for m in ms)) for g, ms in groups.items()}
    halves, totals = ([], []), [(0, 0), (0, 0)]
    for g in sorted(groups, key=lambda g: (-weight[g][0], -weight[g][1], g)):
        side = 0 if totals[0] <= totals[1] else 1
        halves[side].extend(filters[m] for m in sorted(groups[g]))
        totals[side] = (totals[side][0] + weight[g][0], totals[side][1] + weight[g][1])
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


def for_runner(command, runner, cargo_profile=None):
    """A libtest selection as the runner's commands; doctests stay on cargo test.

    A selection with no target filter (`--lib`, `--bins`, `--test`) runs its
    packages' doctests too under cargo test, so under nextest it keeps a
    `cargo test --doc` companion for them.
    """
    cargo = command + (['--profile', cargo_profile] if cargo_profile else [])
    if runner != 'nextest' or '--doc' in command:
        return [cargo]
    assert command[:3] == ['cargo', 'test', '--no-fail-fast'], command
    # nextest's own --profile is the .config/nextest.toml one; cargo test
    # accepts an empty selection, so nextest must too.
    nextest = (['cargo', 'nextest', 'run', '--no-fail-fast', '--profile', NEXTEST_PROFILE,
                '--no-tests', 'warn'] + command[3:] +
               (['--cargo-profile', cargo_profile] if cargo_profile else []))
    if TARGET_FILTERS.isdisjoint(command):
        return [nextest, cargo[:3] + ['--doc'] + cargo[3:]]
    return [nextest]


def totals(lines):
    """Passed/failed/ignored over libtest results and nextest summaries.

    A libtest binary counts its LAST result line: tests that re-run their own
    binary as a child (with a filter) print the child's result line first.
    libtest lines count only while cargo test is the runner: from a `+ cargo
    test` echo until nextest output or a `+ cargo nextest` echo. Only an echo
    switches back, so a child's `Running`/`test result:` lines in nextest's
    captured output count nowhere. A nextest summary's skipped count, less the
    filtered-out tests its marker line names, is the run's #[ignore] count.
    """
    result = dict(passed=0, failed=0, ignored=0)
    sections, section, cargo, filtered = {}, 0, True, 0
    for line in lines:
        line = ANSI_RE.sub('', line)
        if line.startswith('+ '):
            cargo = not line.startswith('+ cargo nextest ')
            filtered = 0 if cargo else filtered
        elif match := FILTERED_RE.search(line):
            filtered = int(match.group(1))
        elif match := NEXTEST_RE.search(line):
            cargo = False
            for count, kind in NEXTEST_PART_RE.findall(match.group(1)):
                key = {'skipped': 'ignored', 'timed out': 'failed', 'exec failed': 'failed'}.get(kind, kind)
                result[key] += int(count) - (filtered if kind == 'skipped' else 0)
            filtered = 0
        elif NEXTEST_OUTPUT_RE.search(line):
            cargo = False
        elif not cargo:
            continue
        elif LIBTEST_HEADER_RE.search(line):
            section += 1
        elif match := LIBTEST_RE.search(line):
            sections[section] = [int(value) for value in match.groups()]
    for counts in sections.values():
        for key, value in zip(('passed', 'failed', 'ignored'), counts):
            result[key] += value
    return result


def keep_junit(target_dir, shard, index):
    """Move the `ci` profile's report aside so the next invocation can't overwrite it."""
    report = Path(target_dir) / 'nextest' / NEXTEST_PROFILE / 'junit.xml'
    if not report.exists():
        return None
    kept = report.with_name(f'junit-{shard}-{index}.xml')
    report.replace(kept)
    return kept


def unit_kind(target):
    if LIB_KINDS & set(target['kind']):
        return 'lib'
    return 'bin' if 'bin' in target['kind'] else target['kind'][0]


def units(package):
    """What `cargo test` runs for a package by default: every test-enabled target, plus doctests."""
    found = {(package['name'], unit_kind(t), t['name'])
             for t in package['targets'] if t.get('test') and 'custom-build' not in t['kind']}
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


def union_problems(packages, budgets):
    """(gaps, repeats): targets cargo test --workspace runs that no shard
    selects, and targets more than one shard selects.

    lib-1 runs the same unit binary as lib-0 under the other half's filters
    (checked disjoint when the lanes run), so the pair counts once.
    trace:TASK-1979 | ai:claude
    """
    seen = {}
    for shard in SHARDS:
        if shard == 'lib-1':
            continue
        for command in commands(shard, packages, budgets):
            for unit in selected(command, packages):
                seen[unit] = seen.get(unit, 0) + 1
    required = set().union(*map(units, packages))
    return sorted(required - set(seen)), sorted(u for u, n in seen.items() if n > 1)


def lib_test_names(root, profile):
    command = ['cargo', 'test'] + LIB_UNIT + profile + ['--', '--list', '--format', 'terse']
    print('+ ' + ' '.join(command), flush=True)
    # The listing's own "Running" line would read as an unfinished binary in the timings log.
    listing = subprocess.run(command, cwd=root, capture_output=True, text=True)
    if listing.returncode:
        sys.stderr.write(listing.stderr)
        raise ValueError(f'listing the lib tests exited {listing.returncode}')
    return [line[:-len(': test')] for line in listing.stdout.splitlines() if line.endswith(': test')]


RUNNING_RE = re.compile(r'(\d{4}-\d\d-\d\dT[\d:]+(?:\.\d+)?)Z running (\d+) tests?$')
DONE_RE = re.compile(r'(\d{4}-\d\d-\d\dT[\d:]+(?:\.\d+)?)Z test (\S+) \.\.\. (?:ok|FAILED|ignored)')


def stamp(text):
    return datetime.datetime.fromisoformat(text[:26])


def lib_test_seconds(log, threads):
    """Per-test seconds from a timestamped lib-lane job log (`gh run view --job N --log`).

    libtest hands tests out in name order to `threads` workers, so test k
    starts when the (k - threads)th completion frees a worker; its time is
    its completion minus that start.
    trace:TASK-1979 | ai:claude
    """
    lines = log.splitlines()
    start = next(stamp(m[1]) for m in map(RUNNING_RE.search, lines) if m and int(m[2]) > 1000)
    finish = {}
    for m in map(DONE_RE.search, lines):
        if m and stamp(m[1]) >= start:
            finish.setdefault(m[2], stamp(m[1]))
    freed = sorted(finish.values())
    return {name: max(0.0, (finish[name] - (start if k < threads else freed[k - threads])).total_seconds())
            for k, name in enumerate(sorted(finish))}


def lib_weights(paths, threads):
    """TOML lines: each module's summed test seconds, the max over the logs."""
    best = collections.Counter()
    for path in paths:
        per_module = collections.Counter()
        for name, secs in lib_test_seconds(Path(path).read_text(errors='replace'), threads).items():
            per_module[module_of(name)] += secs
        for module, secs in per_module.items():
            best[module] = max(best[module], secs)
    return [f'"{m}" = {round(s, 1)}' for m, s in sorted(best.items()) if round(s, 1) >= 0.1]


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
    mode.add_argument('--totals', metavar='LOG', nargs='+')
    mode.add_argument('--lib-weights', nargs='+', metavar='JOB_LOG',
                      help='print [lib_module_seconds] entries from timestamped lib-lane job logs')
    parser.add_argument('--profile')
    parser.add_argument('--runner', choices=('cargo', 'nextest'), default='cargo')
    parser.add_argument('--summary', metavar='OUT', help='append a Markdown totals line (with --totals)')
    parser.add_argument('--threads', type=int, default=4, help="the logs' test threads (CI runners: 4)")
    args = parser.parse_args(argv)
    if args.lib_weights:
        print('\n'.join(lib_weights(args.lib_weights, args.threads)))
        return 0
    if args.totals:
        lines = [line for log in args.totals for line in Path(log).read_text(errors='replace').splitlines()]
        counted = totals(lines)
        counted['passed+ignored'] = counted['passed'] + counted['ignored']
        print(json.dumps(counted))
        if args.summary:
            with open(args.summary, 'a') as handle:
                handle.write('Test totals: ' + ', '.join(f'{v} {k}' for k, v in counted.items()) + '\n')
        return 0
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
        timings = tomllib.load(handle)
    budgets = timings['shard_seconds']
    module_seconds = timings.get('lib_module_seconds', {})
    gaps, repeats = union_problems(packages, budgets)
    if gaps:
        print('::error::no test shard runs these cargo test --workspace targets: ' +
              ', '.join(':'.join(gap) for gap in gaps), file=sys.stderr)
    if repeats:
        print('::error::more than one test shard runs these targets: ' +
              ', '.join(':'.join(unit) for unit in repeats), file=sys.stderr)
    if gaps or repeats:
        return 1
    profile = ['--profile', args.profile] if args.profile else []
    status, reports = 0, 0
    for shard in SHARDS if args.shard == 'all' else [args.shard]:
        if shard == 'lib-1' and args.shard == 'all':
            continue  # lib-0 ran the whole binary.
        for selection in commands(shard, packages, budgets):
            filters = []
            if shard.startswith('lib-') and args.shard != 'all':
                try:
                    names = lib_test_names(root, profile)
                    filters = lib_halves(names, module_seconds)[int(shard[-1])]
                except ValueError as err:
                    print(f'::error::cannot plan {shard}: {err}', file=sys.stderr)
                    return 1
                # nextest reports the other half's tests as skipped; --totals
                # takes this count back out. trace:TASK-1957 | ai:claude
                excluded = len(names) - len(matched(filters, names))
                print(f'# {shard}: {len(filters)} module filters, {excluded} tests filtered out', flush=True)
            for command in for_runner(selection, args.runner, args.profile):
                # Both runners take libtest-style substring filters after `--`.
                command += ['--'] + filters if filters else []
                print('+ ' + ' '.join(command), flush=True)
                code = subprocess.call(command, cwd=root)
                if code:
                    status = 1  # --no-fail-fast also spans separate cargo invocations.
                if command[1] == 'nextest':
                    reports += keep_junit(metadata['target_directory'], shard, reports) is not None
    return status


if __name__ == '__main__':
    sys.exit(main())
