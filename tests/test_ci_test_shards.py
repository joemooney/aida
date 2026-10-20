#!/usr/bin/env python3
"""Coverage, failure, and safety regressions for CI fan-out.
trace:TASK-1949 | ai:codex
"""
import importlib.util
import pathlib
import subprocess
import tempfile
import unittest
import tomllib
from unittest.mock import patch

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('ci_shards', ROOT / 'scripts/ci-test-shards.py')
shards = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shards)
spec = importlib.util.spec_from_file_location('ci_gates', ROOT / 'scripts/check-ci-gates.py')
gates = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gates)


def target(name, kind='test'):
    return dict(name=name, kind=[kind], test=True, src_path=f'/repo/tests/{name}.rs')


class ShardsTest(unittest.TestCase):
    def test_cli_partition_is_complete_disjoint_and_stable(self):
        targets = [target(n) for n in ['d', 'a', 'c', 'b', 'new_test']]
        budgets = {'b:tests/b.rs': 100, 'c:tests/c.rs': 70}
        a, b = shards.cli_halves(targets, budgets)
        self.assertEqual([t['name'] for t in a], ['b', 'a', 'new_test'])
        self.assertEqual([t['name'] for t in b], ['c', 'd'])
        self.assertEqual(shards.cli_halves(list(reversed(targets)), budgets), (a, b))
        self.assertEqual(sorted(t['name'] for t in a+b), sorted(t['name'] for t in targets))

    def test_selections_include_unit_binary_integration_and_doctests(self):
        packages = [dict(name='aida-cli', targets=[target('aida', 'bin'), target('one'), target('two')]),
                    dict(name='aida-cli-lib', targets=[target('lib', 'lib'), target('future')])]
        plan = {s: shards.commands(s, packages, {}) for s in shards.SHARDS}
        self.assertEqual(plan['lib'], [['cargo', 'test', '--no-fail-fast', '-p', 'aida-cli-lib', '--lib']])
        cli = plan['cli-0'][0] + plan['cli-1'][0]
        self.assertEqual(cli.count('--test'), 2)
        rest = plan['rest']
        self.assertIn('--workspace', rest[0])
        self.assertEqual(rest[0][4:8], ['--exclude', 'aida-cli', '--exclude', 'aida-cli-lib'])
        self.assertIn('aida-core/postgres,aida-core/gitlab,aida-core/github,aida-core/jira', rest[0])
        self.assertEqual(rest[1][-3:], ['-p', 'aida-cli', '--bins'])
        self.assertEqual(rest[2][-3:], ['-p', 'aida-cli-lib', '--doc'])
        self.assertEqual(rest[3][-2:], ['--test', 'future'])

    def test_rest_keeps_core_features_supplied_by_excluded_cli(self):
        manifest = tomllib.loads((ROOT / 'aida-cli-lib/Cargo.toml').read_text())
        required = set(manifest['dependencies']['aida-core']['features'])
        packages = [dict(name='aida-cli', targets=[]), dict(name='aida-cli-lib', targets=[])]
        command = shards.commands('rest', packages, {})[0]
        supplied = {f.split('/', 1)[1] for f in command[command.index('--features') + 1].split(',')}
        self.assertLessEqual(required, supplied)

    def test_compile_mode_runs_no_tests_and_preserves_failure(self):
        with patch.object(shards.subprocess, 'call', return_value=19) as call:
            self.assertEqual(shards.main(['--compile', '--profile', 'agent']), 19)
            self.assertEqual(call.call_args.args[0], ['cargo', 'test', '--workspace', '--no-run', '--profile', 'agent'])

    def test_workflow_aggregate_fails_unless_all_lanes_succeed(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = workflow['jobs']['build']
        self.assertEqual(build['strategy']['matrix']['shard'], [*shards.SHARDS, 'gates', 'contracts'])
        self.assertFalse(build['strategy']['fail-fast'])
        aggregate = workflow['jobs']['build-result']
        self.assertEqual(aggregate['name'], 'Build (ubuntu-latest)')
        self.assertEqual(aggregate['needs'], 'build')
        self.assertEqual(aggregate['if'], '${{ always() }}')
        command = aggregate['steps'][0]['run']
        for status in ('success', 'failure', 'cancelled', 'skipped'):
            result = subprocess.run(['bash', '-c', command], env={'RESULT': status})
            self.assertEqual(result.returncode == 0, status == 'success')
        steps = {s.get('name'): s for s in build['steps']}
        source = steps['Run tests']['run']
        self.assertEqual(gates.derive_compile_command('Run tests', source), 'python3 scripts/ci-test-shards.py --compile --profile agent')
        self.assertIn('--shard all --profile agent', gates.local_test_command(source))
        self.assertIn('!cancelled()', steps['Test shard root hygiene']['if'])

    def test_root_guard_detects_loose_files_and_directories(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            subprocess.run(['git', 'init', '-q', tmp], check=True)
            (root / 'file').write_text('tracked')
            subprocess.run(['git', '-C', tmp, 'add', 'file'], check=True)
            self.assertEqual(shards.check_root(root), 0)
            (root / 'leak').write_text('oops')
            self.assertEqual(shards.check_root(root), 1)
            (root / 'leak').unlink()
            (root / 'stray').mkdir()
            (root / 'stray' / 'file').write_text('oops')
            self.assertEqual(shards.check_root(root), 1)

    def test_store_guard_accepts_an_actual_attached_store(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        body = next(s['run'] for s in workflow['jobs']['build']['steps']
                    if 'git -C .aida-store reset --hard' in s.get('run', ''))
        guard = body[body.index('if ['):body.index('\nBASE=')]
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            def git(*args):
                return subprocess.run(['git', '-C', tmp, *args], check=True, capture_output=True)
            git('init', '-q')
            (root / 'sentinel').write_text('main checkout')
            git('add', 'sentinel')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test',
                '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'fixture')
            git('update-ref', 'refs/remotes/origin/aida-store', 'HEAD')
            git('worktree', 'add', '--detach', '.aida-store', 'HEAD')
            result = subprocess.run(['bash', '-e', '-c', guard], cwd=tmp, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'sentinel').read_text(), 'main checkout')

    def test_both_store_guards_refuse_plain_directory_without_reset(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        bodies = [s['run'] for s in workflow['jobs']['build']['steps']
                  if 'git -C .aida-store reset --hard' in s.get('run', '')]
        self.assertEqual(len(bodies), 2)
        for body in bodies:
            guard = body[body.index('          if '):] if '          if ' in body else body[body.index('if ['):]
            guard = guard[:guard.index('\nBASE=')]
            with tempfile.TemporaryDirectory() as tmp:
                root = pathlib.Path(tmp)
                subprocess.run(['git', 'init', '-q', tmp], check=True)
                (root / '.aida-store').mkdir()
                result = subprocess.run(['bash', '-e', '-c', guard], cwd=tmp, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('not an attached git worktree', result.stdout)
                self.assertNotIn('reset', result.stderr)


class TallyTest(unittest.TestCase):
    def test_old_and_sharded_runs_count_test_steps_not_aggregate(self):
        spec = importlib.util.spec_from_file_location('ci_tally', ROOT / 'scripts/ci-flake-tally.py')
        tally = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(tally)
        def job(name, result, ident=1):
            return dict(name=name, id=ident, steps=[] if result is None else [dict(name='Run tests', conclusion=result)])
        cases = [
            ([job('Build (ubuntu-latest)', 'success')], ('pass', [])),
            ([job('Build (ubuntu-latest)', None), job('CI (lib)', 'success'), job('CI (rest)', 'failure', 2)], ('fail', [2])),
            ([job('Build (ubuntu-latest)', None)] + [job(f'CI ({lane})', 'skipped') for lane in shards.SHARDS], ('docs-only', [])),
            ([job('CI (lib)', 'success'), job('CI (rest)', 'skipped', 2)], ('incomplete', [])),
            ([job(f'CI ({lane})', 'success') for lane in shards.SHARDS] + [job('CI (gates)', 'skipped'), job('CI (contracts)', 'skipped')], ('pass', [])),
            ([job('CI (lib)', 'failure'), job('CI (rest)', 'failure', 2)], ('fail', [1, 2])),
        ]
        for jobs, expected in cases:
            with patch.object(tally, 'gh_json', return_value={'jobs': jobs}):
                self.assertEqual(tally.classify({'id': 123}), expected)


if __name__ == '__main__':
    unittest.main()
