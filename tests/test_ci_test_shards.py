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

    def test_lib_halves_balance_measured_seconds_not_counts(self):
        names = [f'slow::t{i}' for i in range(2)] + [f'fast::t{i}' for i in range(50)] + \
                [f'mid::t{i}' for i in range(10)] + ['new::t0']
        a, b = shards.lib_halves(names, {'slow': 100.0, 'fast': 5.0, 'mid': 90.0})
        self.assertEqual({tuple(a), tuple(b)}, {('slow::',), ('fast::', 'mid::', 'new::')})
        self.assertEqual(shards.lib_halves(names, {'slow': 100.0, 'fast': 5.0, 'mid': 90.0}), (a, b))

    def test_lib_weights_read_test_times_from_dispatch_order(self):
        # Two workers: a and b start at once; c starts when a (the first
        # completion) frees a worker; d when b does.
        log = '\n'.join([
            'CI (lib-0)\tRun tests\t2026-10-20T08:00:00.0000000Z running 1001 tests',
            'CI (lib-0)\tRun tests\t2026-10-20T08:00:02.0000000Z test x::a ... ok',
            'CI (lib-0)\tRun tests\t2026-10-20T08:00:05.0000000Z test x::b ... ok',
            'CI (lib-0)\tRun tests\t2026-10-20T08:00:06.0000000Z test y::c ... FAILED',
            'CI (lib-0)\tRun tests\t2026-10-20T08:00:09.0000000Z test y::d ... ignored',
        ])
        self.assertEqual(shards.lib_test_seconds(log, threads=2),
                         {'x::a': 2.0, 'x::b': 5.0, 'y::c': 4.0, 'y::d': 4.0})
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'lib-0.log'
            path.write_text(log)
            self.assertEqual(shards.lib_weights([str(path)], 2), ['"x" = 7.0', '"y" = 8.0'])

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
        # trace:TASK-1957 | ai:claude
        with patch.object(shards, 'lib_test_names', return_value=names), \
                patch.object(shards.subprocess, 'call', return_value=0) as call, \
                patch.object(shards, 'keep_junit') as keep:
            self.assertEqual(shards.main(['--shard', 'lib-1', '--runner', 'nextest', '--profile', 'agent']), 0)
        command = call.call_args.args[0]
        self.assertEqual(command[:3], ['cargo', 'nextest', 'run'])
        self.assertEqual(command[command.index('--') + 1:], list(shards.lib_halves(names)[1]))
        self.assertEqual(command[command.index('--') - 2:command.index('--')], ['--cargo-profile', 'agent'])
        self.assertEqual(keep.call_args.args[1:], ('lib-1', 0))

    def test_union_guard_accepts_the_workspace_and_names_gaps(self):
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT))
        packages = [p for p in metadata['packages'] if p['id'] in metadata['workspace_members']]
        self.assertEqual(shards.union_problems(packages, {}), ([], []))
        lib = dict(target('lib', 'lib'), doctest=True)
        packages = [dict(name='aida-cli', targets=[target('aida', 'bin'), target('one'), target('two'),
                                                   dict(lib, name='cli'), target('macros', 'proc-macro')]),
                    dict(name='aida-cli-lib', targets=[lib, target('tool', 'bin'), target('ex', 'example')]),
                    dict(name='aida-core', targets=[dict(lib, name='core')])]
        gaps, repeats = shards.union_problems(packages, {})
        self.assertEqual(gaps, [
            ('aida-cli', 'doc', 'cli'), ('aida-cli', 'lib', 'cli'), ('aida-cli', 'lib', 'macros'),
            ('aida-cli-lib', 'bin', 'tool'), ('aida-cli-lib', 'example', 'ex')])
        self.assertEqual(repeats, [])
        with patch.object(shards, 'union_problems', return_value=([('aida-cli', 'lib', 'cli')], [])), \
                patch.object(shards.subprocess, 'call', return_value=0) as call:
            self.assertEqual(shards.main(['--shard', 'rest']), 1)
        call.assert_not_called()

    def test_union_guard_refuses_a_target_two_shards_run(self):
        packages = [dict(name='aida-cli', targets=[target('aida', 'bin'), target('one'), target('two')]),
                    dict(name='aida-cli-lib', targets=[dict(target('lib', 'lib'), doctest=True)])]
        real = shards.commands

        def overlapping(shard, pkgs, budgets):
            # A rest lane that also ran every CLI integration target.
            extra = [['cargo', 'test', '-p', 'aida-cli', '--test', 'one']] if shard == 'rest' else []
            return real(shard, pkgs, budgets) + extra
        with patch.object(shards, 'commands', side_effect=overlapping):
            gaps, repeats = shards.union_problems(packages, {})
        self.assertEqual(gaps, [])
        self.assertEqual(repeats, [('aida-cli', 'test', 'one')])
        with patch.object(shards, 'union_problems', return_value=([], [('aida-cli', 'test', 'one')])), \
                patch.object(shards.subprocess, 'call', return_value=0) as call:
            self.assertEqual(shards.main(['--shard', 'cli-0']), 1)
        call.assert_not_called()

    def test_check_ci_list_labels_run_tests_with_its_real_local_command(self):
        listing = subprocess.run(['python3', 'scripts/check-ci-gates.py', '--list', '--tier', 'all'],
                                 cwd=ROOT, capture_output=True, text=True, check=True).stdout
        line = next(l for l in listing.splitlines() if l.startswith('PLAN  Run tests  '))
        self.assertIn('(local: python3 scripts/ci-test-shards.py --shard all --profile agent', line)
        self.assertNotIn('(local: cargo test --profile agent)', listing)

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

    # trace:TASK-1957 | ai:claude
    def test_nextest_runner_keeps_selection_and_leaves_doctests_on_cargo_test(self):
        packages = [dict(name='aida-cli', targets=[target('one'), target('two')]),
                    dict(name='aida-cli-lib', targets=[target('lib', 'lib')])]
        docs = 0
        for shard in shards.SHARDS:
            for command in shards.commands(shard, packages, {}):
                self.assertEqual(shards.for_runner(command, 'cargo'), [command])
                rewritten = shards.for_runner(command, 'nextest', 'agent')
                if '--doc' in command:
                    self.assertEqual(rewritten, [command + ['--profile', 'agent']])
                    docs += 1
                    continue
                nextest = rewritten[0]
                self.assertEqual(nextest[:6], ['cargo', 'nextest', 'run', '--no-fail-fast', '--profile', 'ci'])
                self.assertEqual(nextest[8:], command[3:] + ['--cargo-profile', 'agent'])
                if '--workspace' in command:
                    # Its packages' doctests ran under cargo test; they still must.
                    self.assertEqual(rewritten[1], command[:3] + ['--doc'] + command[3:] + ['--profile', 'agent'])
                    docs += 1
                else:
                    self.assertEqual(len(rewritten), 1)
        self.assertEqual(docs, 2)

    def test_totals_count_libtest_and_nextest_logs_alike(self):
        log = ['     Running unittests src/lib.rs (target/debug/deps/x-0123)',
               # A test that re-runs its own binary as a child prints the child's result first.
               'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.0s',
               'test result: ok. 5 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.0s',
               '   Doc-tests x',
               'test result: ok. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.0s',
               '\x1b[32;1m     Summary\x1b[0m [  12.345s] 126 tests run: 120 passed (3 slow), 2 failed, '
               '1 timed out, 4 skipped',
               '     Summary [   1.0s] 8/10 tests run: 8 passed, 0 skipped']
        self.assertEqual(shards.totals(log), dict(passed=134, failed=4, ignored=6))
        # A child's result in nextest's captured failure output, after a
        # cargo-test section, counts nowhere and leaves the section's count.
        log = ['     Running unittests src/lib.rs (target/debug/deps/x-0123)',
               'test result: ok. 100 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.0s',
               '--- STDOUT:              x foo::child ---',
               ' test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.0s',
               '     Summary [   1.0s] 5 tests run: 4 passed, 1 failed, 0 skipped']
        self.assertEqual(shards.totals(log), dict(passed=104, failed=1, ignored=0))
        log[2] = '+ cargo nextest run --no-fail-fast --profile ci -p x --bins'
        self.assertEqual(shards.totals(log), dict(passed=104, failed=1, ignored=0))

    def test_each_nextest_invocation_keeps_its_own_junit_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = pathlib.Path(tmp) / 'nextest' / shards.NEXTEST_PROFILE / 'junit.xml'
            report.parent.mkdir(parents=True)
            self.assertIsNone(shards.keep_junit(tmp, 'rest', 0))
            for index in range(3):
                report.write_text(f'<run {index}/>')
                self.assertEqual(shards.keep_junit(tmp, 'rest', index).name, f'junit-rest-{index}.xml')
            self.assertFalse(report.exists())
            self.assertEqual(sorted(p.read_text() for p in report.parent.glob('junit-*.xml')),
                             ['<run 0/>', '<run 1/>', '<run 2/>'])

    def test_nextest_shadow_job_is_parallel_and_non_gating(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        shadow = workflow['jobs']['nextest-shadow']
        self.assertNotIn('needs', shadow)
        self.assertNotIn('nextest-shadow', workflow['jobs']['build-result']['needs'])
        self.assertEqual(shadow['strategy']['matrix']['shard'], list(shards.SHARDS))
        steps = {s.get('name'): s for s in shadow['steps']}
        self.assertTrue(steps['Run tests under nextest']['continue-on-error'])
        install = steps['Install cargo-nextest']
        self.assertRegex(install['uses'], r'^taiki-e/install-action@[0-9a-f]{40}$')
        self.assertRegex(install['with']['tool'], r'^cargo-nextest@\d+\.\d+\.\d+$')
        self.assertFalse(steps['Cache Rust dependencies']['with']['save-if'])
        config = tomllib.loads((ROOT / '.config/nextest.toml').read_text())['profile'][shards.NEXTEST_PROFILE]
        self.assertEqual(config['retries'], 0)
        self.assertEqual(config['slow-timeout']['period'], '10s')
        self.assertIn('terminate-after', config['slow-timeout'])
        self.assertEqual(config['junit']['path'], 'junit.xml')
        upload = steps['Upload nextest report']['with']['path']
        self.assertIn('target/nextest/ci/junit-*.xml', upload)
        nextest = tomllib.loads((ROOT / '.config/nextest.toml').read_text())
        self.assertEqual(nextest['test-groups']['serial'], {'max-threads': 1})
        self.assertTrue(config['overrides'])
        self.assertTrue(all(o['test-group'] == 'serial' for o in config['overrides']))

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
