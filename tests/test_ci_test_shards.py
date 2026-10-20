#!/usr/bin/env python3
"""Coverage, failure, and safety regressions for CI fan-out.
trace:TASK-1949 | ai:codex
"""
import importlib.util
import json
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
        for half in ('lib-0', 'lib-1'):
            self.assertEqual(plan[half], [['cargo', 'test', '--no-fail-fast', '-p', 'aida-cli-lib', '--lib']])
        cli = plan['cli-0'][0] + plan['cli-1'][0]
        self.assertEqual(cli.count('--test'), 2)
        rest = plan['rest']
        self.assertIn('--workspace', rest[0])
        self.assertEqual(rest[0][4:8], ['--exclude', 'aida-cli', '--exclude', 'aida-cli-lib'])
        self.assertIn('aida-core/postgres,aida-core/gitlab,aida-core/github,aida-core/jira', rest[0])
        self.assertEqual(rest[1][-3:], ['-p', 'aida-cli', '--bins'])
        self.assertEqual(rest[2][-3:], ['-p', 'aida-cli-lib', '--doc'])
        self.assertEqual(rest[3][-2:], ['--test', 'future'])

    # trace:TASK-1979 | ai:claude
    def test_lib_halves_are_a_disjoint_balanced_cover(self):
        # `leases::` also matches ship::leases::q and `tests::` matches cli::tests::b,
        # so those modules must share a half, or a test would run in both.
        names = ['tests::a', 'cli::tests::b', 'cli::x', 'ship::leases::q', 'leases::z',
                 'leases::w', 'other::y', 'other::v', 'root_test']
        a, b = shards.lib_halves(names)
        self.assertEqual(shards.lib_halves(list(reversed(names))), (a, b))
        ran_a, ran_b = shards.matched(a, names), shards.matched(b, names)
        self.assertFalse(ran_a & ran_b)
        self.assertEqual(ran_a | ran_b, set(names))
        for together in (['tests::', 'cli::'], ['leases::', 'ship::']):
            self.assertTrue(set(together) <= set(a) or set(together) <= set(b))
        self.assertIn('root_test', a + b)
        self.assertLessEqual(abs(len(ran_a) - len(ran_b)), 3)

    def test_lib_halves_refuses_an_unsplittable_binary(self):
        with self.assertRaises(ValueError):
            shards.lib_halves(['only::one'])

    def test_lib_lane_runs_its_half_as_filters(self):
        names = ['a::one', 'b::two', 'b::three']
        with patch.object(shards, 'lib_test_names', return_value=names), \
                patch.object(shards.subprocess, 'call', return_value=0) as call:
            self.assertEqual(shards.main(['--shard', 'lib-1', '--profile', 'agent']), 0)
        command = call.call_args.args[0]
        self.assertEqual(command[command.index('--') + 1:], list(shards.lib_halves(names)[1]))
        self.assertEqual(command[command.index('--') - 2:command.index('--')], ['--profile', 'agent'])

    def test_union_guard_accepts_the_workspace_and_names_gaps(self):
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT))
        packages = [p for p in metadata['packages'] if p['id'] in metadata['workspace_members']]
        self.assertEqual(shards.union_gaps(packages, {}), [])
        lib = dict(target('lib', 'lib'), doctest=True)
        packages = [dict(name='aida-cli', targets=[target('aida', 'bin'), target('one'), target('two'), dict(lib, name='cli')]),
                    dict(name='aida-cli-lib', targets=[lib, target('tool', 'bin'), target('ex', 'example')]),
                    dict(name='aida-core', targets=[dict(lib, name='core')])]
        self.assertEqual(shards.union_gaps(packages, {}), [
            ('aida-cli', 'doc', 'cli'), ('aida-cli', 'lib', 'cli'),
            ('aida-cli-lib', 'bin', 'tool'), ('aida-cli-lib', 'example', 'ex')])
        with patch.object(shards, 'union_gaps', return_value=[('aida-cli', 'lib', 'cli')]), \
                patch.object(shards.subprocess, 'call', return_value=0) as call:
            self.assertEqual(shards.main(['--shard', 'rest']), 1)
        call.assert_not_called()

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
        self.assertEqual(steps['Build aida binary for lib unit tests']['if'],
                         "${{ startsWith(matrix.shard, 'lib-') && (steps.changes.outputs.full_ci == 'true') }}")
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
            ([job(f'CI ({lane})', 'success') for lane in ('lib', 'cli-0', 'cli-1', 'rest')], ('pass', [])),
            ([job(f'CI ({lane})', 'success') for lane in ('lib-0', 'cli-0', 'cli-1', 'rest')], ('incomplete', [])),
            ([job('CI (lib)', 'failure'), job('CI (rest)', 'failure', 2)], ('fail', [1, 2])),
        ]
        for jobs, expected in cases:
            with patch.object(tally, 'gh_json', return_value={'jobs': jobs}):
                self.assertEqual(tally.classify({'id': 123}), expected)


if __name__ == '__main__':
    unittest.main()
