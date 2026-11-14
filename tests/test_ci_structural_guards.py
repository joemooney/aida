"""FAST/FULL shared structural-test selection and mutation coverage.
trace:TASK-3791 | ai:codex
"""
import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / 'aida-ci-guards/lib.rs'
# Selection lives in the harness's #[path] list. Explicit behavioral-test
# exclusions are per test, so adding a ratchet to a mixed module still fails.
EXCLUSIONS = ROOT / 'scripts/ci-structural-exclusions.json'
_spec = importlib.util.spec_from_file_location('store_writer_mask', ROOT / 'scripts/check-store-writers.py')
_mask = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mask)
TEST = re.compile(r'#\[test\](?:(?!\bfn\b).)*?\bfn\s+(\w+)', re.S)


def candidates(root):
    result = set()
    for source in root.glob('aida-*/src/**/*.rs'):
        path = source.relative_to(root).as_posix()
        module = 'ratchet' in source.stem or source.stem.endswith('_inventory_tests')
        text = source.read_text()
        if not module and not re.search(r'fn\s+\w*(?:ratchet|inventor)\w*', text):
            continue
        for name in TEST.findall(_mask.mask_rust(text)):
            if module or any(word in name for word in ('ratchet', 'inventor')):
                result.add(f'{path}::{name}')
    return result


def selected():
    return set(re.findall(r'#\[path = "../(aida-cli-lib/src/(?:gate/)?tests/[^"\n]+)"\]',
                          HARNESS.read_text()))


def missing(root, paths, exclusions):
    return {test for test in candidates(root)
            if test.rsplit('::', 1)[0] not in paths and test not in exclusions}


class StructuralSelection(unittest.TestCase):
    def test_selection_is_a_checked_cover(self):
        paths = selected()
        exclusions = json.loads(EXCLUSIONS.read_text())
        self.assertGreaterEqual(len(paths), 8)
        self.assertTrue(all(reason.strip() for reason in exclusions.values()))
        self.assertEqual(missing(ROOT, paths, exclusions), set(),
                         'Add the source module to aida-ci-guards/lib.rs or justify an explicit exclusion')
        self.assertEqual(set(exclusions) - candidates(ROOT), set(), 'Remove stale exclusions')
        self.assertFalse(any(test.rsplit('::', 1)[0] in paths for test in exclusions))
        for path in paths:
            self.assertTrue((ROOT / path).is_file(), path)

    def test_new_ratchet_or_inventory_test_requires_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'aida-cli-lib/src/tests/new_tests.rs'
            source.parent.mkdir(parents=True)
            source.write_text('#[test]\nfn new_ratchet() {}\n#[test]\nfn new_inventory() {}')
            self.assertEqual(len(missing(root, set(), {})), 2)
            self.assertEqual(missing(root, {source.relative_to(root).as_posix()}, {}), set())

    def test_fast_runs_the_shared_harness_and_selection_audit(self):
        import tomllib
        import yaml
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        step = next(s for s in workflow['jobs']['build']['steps']
                    if s.get('name') == 'Rust structural ratchets')
        self.assertEqual(step['timeout-minutes'], 2)
        self.assertIn('python3 -m unittest tests/test_ci_structural_guards.py', step['run'])
        self.assertIn('cargo test -p aida-ci-guards --lib', step['run'])
        tiers = tomllib.loads((ROOT / 'scripts/ci-gate-tiers.toml').read_text())
        self.assertEqual(tiers['tiers'][step['name']], 'fast')


if __name__ == '__main__':
    unittest.main()
