"""Checked ratchet selection and negative controls. trace:STORY-2833 | ai:codex"""
import importlib.util
import json
from pathlib import Path
import unittest
import contextlib
import io
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('structural_selection', ROOT / 'tests/test_ci_structural_guards.py')
selection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selection)


# Credential-routing and time-flag classification source guards; FAST, the lane
# pre-push gate and the integration-tip check must all run them (trace:STORY-2833 | ai:claude).
GUARDS = ('task_2001_every_host_remote_call_routes_its_credentials',
          'every_time_bound_flag_is_classified')
REQUIRED = {
    ('aida-cli-lib', 'story_1888_gate_paths'),
    ('aida-cli-lib', 'story_1728_command_registry_tests'),
    ('aida-cli-lib', 'bug_1985_host_status_guard'),
    ('aida-cli', 'checked_in_audit_'),
} | {('aida-cli-lib', guard) for guard in GUARDS}


def names(text):
    return set(selection.TEST.findall(selection._mask.mask_rust(text)))


class RatchetSet(unittest.TestCase):
    def test_runner_refuses_empty_filters_and_failed_children(self):
        runner_spec = importlib.util.spec_from_file_location('ratchet_runner', ROOT / 'scripts/check-ratchets.py')
        runner = importlib.util.module_from_spec(runner_spec)
        runner_spec.loader.exec_module(runner)
        with contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(SystemExit):
                runner.run([sys.executable, '-c', 'print("test result: ok. 0 passed; 0 failed;")'], require_tests=True)
            with self.assertRaises(SystemExit) as failed:
                runner.run([sys.executable, '-c', 'raise SystemExit(7)'])
            self.assertEqual(failed.exception.code, 7)
            with self.assertRaisesRegex(SystemExit, 'expected 2 executed tests, got 1'):
                runner.run([sys.executable, '-c', 'print("test result: ok. 1 passed; 0 failed;")'],
                           require_tests=True, expected_tests=2)
            runner.run([sys.executable, '-c', 'print("test result: ok. 1 passed; 0 failed;")'], require_tests=True)

    def test_checked_modules_do_not_silently_gain_or_lose_tests(self):
        manifest = json.loads((ROOT / 'scripts/ratchet-set.json').read_text())
        self.assertEqual(manifest['version'], 1)
        self.assertTrue(REQUIRED <= {(group['package'], group['filter']) for group in manifest['groups']})
        exclusions = json.loads(selection.EXCLUSIONS.read_text())
        chosen = {}
        for group in manifest['groups']:
            if group['scope'] == 'filter':
                for name in group['tests']:
                    self.assertIn(group['filter'], name, 'cargo filter must select every listed test')
            chosen.setdefault(group['source'], set()).update(group['tests'])
        # Compare every #[test] in a selected module, unfiltered: each needs a
        # selection here or a justified per-test disposition (trace:STORY-2833 | ai:claude).
        for source, listed in chosen.items():
            actual = names((ROOT / source).read_text())
            self.assertTrue(actual, source)
            self.assertEqual(listed - actual, set(), f'{source}: remove stale ratchet selections')
            disposed = {test.rsplit('::', 1)[1] for test in exclusions
                        if test.rsplit('::', 1)[0] == source}
            self.assertEqual(actual - listed - disposed, set(),
                             f'{source}: select each new test in scripts/ratchet-set.json or justify '
                             'it in scripts/ci-structural-exclusions.json')
        self.assertEqual(set(manifest['shared_modules']), selection.selected())
        for path, listed in manifest['shared_modules'].items():
            self.assertEqual(names((ROOT / path).read_text()), set(listed), path)
        covered = {group['source'] + '::' + name for group in manifest['groups']
                   for name in group['tests']}
        for test, reason in json.loads(selection.EXCLUSIONS.read_text()).items():
            if 'covered in FAST after compilation by scripts/ratchet-set.json' in reason:
                self.assertIn(test, covered, 'Application ratchet disposition lost its selection')

    def test_new_test_in_a_ratchet_module_requires_listing(self):
        source = ROOT / 'aida-cli-lib/src/tests/story_1888_gate_paths_tests.rs'
        original = Path.read_text

        def injected(path, *args, **kwargs):
            text = original(path, *args, **kwargs)
            if path == source:
                text += '\n#[test]\nfn new_unlisted_guard() {}\n'
            return text

        with patch.object(Path, 'read_text', injected):
            with self.assertRaisesRegex(AssertionError, 'new_unlisted_guard'):
                self.test_checked_modules_do_not_silently_gain_or_lose_tests()

    # trace:STORY-2833 | ai:claude
    def test_new_test_in_a_mixed_module_requires_selection_or_disposition(self):
        # The name matches no group filter; the old filtered comparison let it through.
        source = ROOT / 'aida-cli-lib/src/tests/bug_2398_codex_tool_env_tests.rs'
        original = Path.read_text

        def injected(path, *args, **kwargs):
            text = original(path, *args, **kwargs)
            return text + '\n#[test]\nfn new_unlisted_behavior() {}\n' if path == source else text

        with patch.object(Path, 'read_text', injected):
            with self.assertRaisesRegex(AssertionError, 'new_unlisted_behavior'):
                self.test_checked_modules_do_not_silently_gain_or_lose_tests()

    # trace:STORY-2833 | ai:claude
    def test_failing_credential_or_time_flag_ratchet_blocks_fast_and_integration_tip(self):
        manifest = json.loads((ROOT / 'scripts/ratchet-set.json').read_text())
        groups = {group['filter']: group for group in manifest['groups']}
        runner_spec = importlib.util.spec_from_file_location('ratchet_runner', ROOT / 'scripts/check-ratchets.py')
        runner = importlib.util.module_from_spec(runner_spec)
        runner_spec.loader.exec_module(runner)
        for test in GUARDS:
            group = groups[test]
            self.assertEqual((group['package'], group['target'], group['tests']),
                             ('aida-cli-lib', ['--lib'], [test]))
            seen = []

            def fake(command, env=None, require_tests=False, expected_tests=None):
                seen.append(command)
                if command[-1] == test:
                    # The guard's cargo test exits non-zero, as on an injected failure.
                    raise SystemExit(101)

            with patch.object(runner, 'run', fake), patch.object(sys, 'argv', ['check-ratchets.py', '--profile', 'dev']):
                with self.assertRaises(SystemExit) as failed:
                    runner.main()
            self.assertEqual(failed.exception.code, 101, test)
            self.assertIn(['cargo', 'test', '--profile', 'dev', '-p', 'aida-cli-lib', '--lib', '--', test], seen)
        # Integration PRs (head `integration`, not `train/`) take FAST, whose
        # fast-compile job runs this runner (see test_fast_compile_and_preflight_run_the_same_set).
        tiers_spec = importlib.util.spec_from_file_location('ci_tiers', ROOT / 'tests/test_ci_tiers.py')
        tiers = importlib.util.module_from_spec(tiers_spec)
        tiers_spec.loader.exec_module(tiers)
        full = tiers.JOBS['tier']['steps'][0]['env']['FULL']
        for action in ('opened', 'synchronize', 'reopened'):
            self.assertEqual(tiers.render(full, action=action, head='integration'), 'False', action)
        self.assertEqual(tiers.JOBS['fast-compile']['if'], "needs.tier.outputs.full != 'true'")
        # The integration-tip check refuses to push when check-ci-fast fails,
        # and check-ci-fast runs this runner.
        sync = (ROOT / 'scripts/integration-sync-main.sh').read_text()
        self.assertRegex(sync, r'(?m)^make check-ci-fast \|\| die "check-ci-fast failed.*NOT pushed')
        makefile = (ROOT / 'Makefile').read_text()
        self.assertIn('check-ci-fast: check-ci-source check-ratchets', makefile)
        self.assertRegex(makefile, r'(?m)^check-ratchets:.*\n\t@python3 scripts/check-ratchets.py$')

    def test_fast_compile_and_preflight_run_the_same_set(self):
        import yaml
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        steps = workflow['jobs']['fast-compile']['steps']
        compile_index = next(i for i, step in enumerate(steps)
                             if 'cargo check --profile dev --workspace --tests' in step.get('run', ''))
        ratchet_index = next(i for i, step in enumerate(steps) if step.get('name') == 'Ratchet set')
        self.assertGreater(ratchet_index, compile_index)
        self.assertEqual(steps[ratchet_index]['run'].strip(), 'python3 scripts/check-ratchets.py --profile dev')
        self.assertIn('"Ratchet set"', (ROOT / 'aida-cli-lib/src/implementer_preflight.rs').read_text())
        import tomllib
        config = tomllib.loads((ROOT / '.aida/config.toml').read_text())
        self.assertIn('Ratchet set', config['preflight']['guards'])
        makefile = (ROOT / 'Makefile').read_text()
        self.assertIn('check-ci-fast: check-ci-source check-ratchets', makefile)
        self.assertIn('python3 scripts/check-ratchets.py', makefile)


if __name__ == '__main__':
    unittest.main()
