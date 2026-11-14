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
        self.assertTrue({
            ('aida-cli-lib', 'story_1888_gate_paths'),
            ('aida-cli-lib', 'story_1728_command_registry_tests'),
            ('aida-cli-lib', 'bug_1985_host_status_guard'),
            ('aida-cli', 'checked_in_audit_'),
        } <= {(group['package'], group['filter']) for group in manifest['groups']})
        for group in manifest['groups']:
            actual = names((ROOT / group['source']).read_text())
            # Mixed runtime suites select their named source guards, not their
            # subprocess/launch behavior fixtures.
            if group['scope'] == 'filter':
                actual = {name for name in actual if group['filter'] in name}
            self.assertTrue(actual, group['source'])
            self.assertEqual(actual, set(group['tests']),
                             'Update the reviewed ratchet set when adding/removing tests')
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
