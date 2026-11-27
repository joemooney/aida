"""check-ci-fast builds nothing; the ratchet set moved to check-ci-compile.

trace:BUG-2782 | ai:claude
"""
import importlib.util
import io
import os
import re
import sys
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
CARGO_STEP = re.compile(r'\bcargo\s+(?:\+\S+\s+)?(?:build|test|check|nextest|run)\b')


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def make_rules():
    """target -> (prerequisites, recipe lines) for every plain Makefile rule."""
    rules, current = {}, None
    for line in (ROOT / 'Makefile').read_text().splitlines():
        rule = re.match(r'^([A-Za-z0-9_.-]+):(?!=)([^#]*)', line)
        if rule:
            current = rule[1]
            rules[current] = (rule[2].split(), [])
        elif line.startswith('\t') and current:
            rules[current][1].append(line.strip())
        elif line.strip() and not line.startswith('#'):
            current = None
    return rules


class CheckCiFastIsBuildFree(unittest.TestCase):
    def test_check_ci_fast_closure_has_no_cargo_or_ratchet_step(self):
        # AC4: resolve every target check-ci-fast reaches and every recipe line.
        rules, seen, frontier = make_rules(), set(), ['check-ci-fast']
        while frontier:
            target = frontier.pop()
            if target in seen:
                continue
            seen.add(target)
            prerequisites, recipe = rules[target]
            frontier.extend(prerequisites)
            for line in recipe:
                self.assertNotRegex(line, CARGO_STEP, f'{target}: {line}')
                self.assertNotIn('check-ratchets', line, target)
                if 'check-ci-gates.py' in line:
                    self.assertIn('--tier fast --no-build', line, target)
        self.assertEqual(seen, {'check-ci-fast'})

    def test_no_build_fast_plan_runs_no_cargo_step(self):
        gates = load('check_ci_gates', 'scripts/check-ci-gates.py')
        ran, stalls = [], []

        def fake_plan(root, tier, plan, tiers, env, stall_secs=0.0):
            ran.extend(plan)
            stalls.append(stall_secs)
            return 0

        out = io.StringIO()
        with mock.patch.object(gates, 'run_plan', fake_plan), redirect_stdout(out):
            self.assertEqual(gates.main(['--tier', 'fast', '--no-build']), 0)
        no_build = list(ran)
        self.assertGreater(len(no_build), 10)
        self.assertGreater(stalls[0], 0)  # AC3: only check-ci-fast arms the watchdog
        with mock.patch.object(gates, 'run_plan', fake_plan), redirect_stdout(io.StringIO()):
            self.assertEqual(gates.main(['--tier', 'fast']), 0)
        self.assertEqual(stalls[-1], 0.0)  # CI's check-ci-source is unchanged
        for name, command in no_build:
            self.assertNotRegex(command, CARGO_STEP, name)
        # The skipped cargo gate is named with where it still runs.
        self.assertRegex(out.getvalue(), r'SKIP  Rust structural ratchets\n\s+runs cargo; `make check-ci-compile`')
        # The skipped gate's cargo half is in the ratchet set check-ci-compile runs.
        runner = (ROOT / 'scripts/check-ratchets.py').read_text()
        self.assertIn("'-p', 'aida-ci-guards', '--lib'", runner)
        self.assertIn('tests/test_ci_structural_guards.py', runner)
        # CI's fast-gates job keeps the full fast tier.
        self.assertEqual((ROOT / 'Makefile').read_text().count(
            'check-ci-source: ## Source-only FAST gates'), 1)
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            gates.main(['--tier', 'all', '--no-build'])

    def test_ratchets_run_in_check_ci_compile_and_integration_sync(self):
        rules = make_rules()
        self.assertIn('check-ratchets', rules['check-ci-compile'][0])
        self.assertEqual(rules['check-ratchets'][1], ['@python3 scripts/check-ratchets.py --manual auto'])
        sync = (ROOT / 'scripts/integration-sync-main.sh').read_text()
        self.assertRegex(sync, r'(?m)^make check-ratchets \|\| die ')

    def test_check_ci_list_names_the_ratchet_placement(self):
        gates = load('check_ci_gates', 'scripts/check-ci-gates.py')
        for argv in (['--tier', 'all', '--list'], ['--tier', 'compile', '--list']):
            out = io.StringIO()
            with redirect_stdout(out):
                self.assertEqual(gates.main(argv), 0)
            self.assertIn('Ratchet set: `make check-ci-compile` runs it', out.getvalue(), argv)
            self.assertIn('not `make check-ci-fast`', out.getvalue(), argv)


class ManualCheckGate(unittest.TestCase):
    def setUp(self):
        self.runner = load('ratchet_runner', 'scripts/check-ratchets.py')

    def test_surface_paths_select_the_manual_check(self):
        reason = self.runner.manual_reason
        # Built from parts: these fixture paths must not read as doc links.
        cli = 'docs/' + 'cli'
        for path in (f'{cli}/04-git.md', 'aida-cli/src/main.rs', 'aida-cli-lib/src/pr_args.rs',
                     'aida-cli-lib/src/lane/job_args.rs', 'aida-cli-lib/src/command_registry.rs',
                     'aida-cli-lib/src/cli.rs', 'aida-cli-lib/src/tests/golden/cli_help_snapshot.txt',
                     'scripts/check-ratchets.py'):
            self.assertIn(path, reason('auto', {path}), path)
        for path in ('aida-core/src/lib.rs', 'aida-cli-lib/src/pr_cmd.rs', f'{cli}.md',
                     'aida-cli-lib/src/cli.rs.bak', f'x{cli}/a.md', ''):
            self.assertIsNone(reason('auto', {path}), path)
        self.assertIsNone(reason('auto', set()))
        self.assertIn('2 more', reason('auto', {f'{cli}/a.md', f'{cli}/b.md', 'aida-cli/x'}))

    def test_unknown_diff_or_always_runs_it(self):
        self.assertIsNotNone(self.runner.manual_reason('auto', None))
        self.assertIsNotNone(self.runner.manual_reason('always', set()))

    def test_git_failure_means_unknown(self):
        failed = mock.Mock(returncode=128, stdout='')
        with mock.patch.object(self.runner.subprocess, 'run', return_value=failed):
            self.assertIsNone(self.runner.changed_paths('no-such-ref'))

    def run_main(self, argv, paths):
        seen = []
        fake = lambda command, **kwargs: seen.append((command, kwargs.get('why')))
        out = io.StringIO()
        with mock.patch.object(self.runner, 'run', fake), \
                mock.patch.object(self.runner, 'changed_paths', return_value=paths), \
                mock.patch.object(sys, 'argv', ['check-ratchets.py', *argv]), redirect_stdout(out):
            self.runner.main()
        return seen, out.getvalue()

    def test_auto_skips_build_without_surface_change_and_ci_default_always_runs(self):
        seen, out = self.run_main(['--manual', 'auto'], {'aida-core/src/lib.rs'})
        self.assertFalse(any(command[:2] == ['cargo', 'build'] for command, _ in seen))
        self.assertIn('skipped aida build and docs/cli/verify-manual.py', out)
        # AC1: every step says what it is and why.
        self.assertTrue(all(why for _, why in seen), seen)
        seen, _ = self.run_main(['--profile', 'dev'], set())
        self.assertIn(['cargo', 'build', '--profile', 'dev', '-p', 'aida-cli', '--bin', 'aida'],
                      [command for command, _ in seen])
        self.assertEqual(seen[-1][0][1], 'docs/cli/verify-manual.py')
        page = 'docs/' + 'cli/04-git.md'
        seen, _ = self.run_main(['--manual', 'auto'], {page})
        self.assertIn(page, seen[-1][1])


@unittest.skipUnless(os.path.isdir('/proc'), 'the stall watchdog reads /proc')
class StallWatchdog(unittest.TestCase):
    def setUp(self):
        self.gates = load('check_ci_gates', 'scripts/check-ci-gates.py')

    def run_gate(self, command, stall_secs):
        out, err = io.StringIO(), io.StringIO()
        started = time.monotonic()
        with redirect_stdout(out), redirect_stderr(err):
            passed, _, _ = self.gates.run_gate(ROOT, 'fixture step', command, {'PATH': os.defpath, 'LC_ALL': 'C'}, stall_secs)
        return passed, time.monotonic() - started, err.getvalue()

    def test_blocked_step_fails_fast_naming_step_and_wait(self):
        # AC3: an idle tree is killed after the stall window, not left to hang.
        passed, elapsed, err = self.run_gate('echo waiting on lock; sleep 60', 1.0)
        self.assertFalse(passed)
        self.assertLess(elapsed, 20)
        self.assertIn('STALL fixture step', err)
        self.assertRegex(err, r'pid \d+ sleep \(')
        self.assertIn('last output: waiting on lock', err)

    def test_busy_or_quick_steps_are_not_stalls(self):
        passed, _, err = self.run_gate('timeout 3 sh -c "while :; do :; done" || true', 1.0)
        self.assertTrue(passed, err)
        self.assertNotIn('STALL', err)
        passed, _, _ = self.run_gate('sleep 2', 0)
        self.assertTrue(passed)

    def test_stall_limit_reads_the_env_knob(self):
        self.assertEqual(self.gates.stall_limit({}), 120.0)
        self.assertEqual(self.gates.stall_limit({'AIDA_GATE_STALL_SECS': '0'}), 0.0)
        self.assertEqual(self.gates.stall_limit({'AIDA_GATE_STALL_SECS': 'x'}), 120.0)


if __name__ == '__main__':
    unittest.main()
