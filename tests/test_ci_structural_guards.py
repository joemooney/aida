"""FAST/FULL shared structural-test selection and mutation coverage.
trace:TASK-3791 | ai:codex
trace:STORY-2833 | ai:codex
"""
import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / 'aida-ci-guards/lib.rs'
# Selection lives in the harness's #[path] list. Explicit behavioral-test
# exclusions are per test, so adding a ratchet to a mixed module still fails.
EXCLUSIONS = ROOT / 'scripts/ci-structural-exclusions.json'
_spec = importlib.util.spec_from_file_location('store_writer_mask', ROOT / 'scripts/check-store-writers.py')
_mask = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mask)
TEST = re.compile(r'#\[test\](?:(?!\bfn\b).)*?\bfn\s+(\w+)', re.S)
RATCHET_SET = ROOT / 'scripts/ratchet-set.json'
MARKERS = ('ratchet', 'inventor')


def ratchet_selections(manifest_path=RATCHET_SET):
    """`source::test` for every application ratchet run in FAST after compilation."""
    manifest = json.loads(manifest_path.read_text())
    return {group['source'] + '::' + name for group in manifest['groups'] for name in group['tests']}


def candidates(root, selections=frozenset()):
    """Every #[test] in a ratchet-kind or mixed module.

    A mixed module holds a ratchet/inventory-named test or a ratchet-set
    selection. All of its tests are candidates, so a new test there needs a
    selection or a justified per-test disposition; no name is silently ignored.
    trace:STORY-2833 | ai:claude
    """
    result = set()
    reviewed = {test.rsplit('::', 1)[0] for test in selections}
    sources = list(root.glob('aida-*/src/**/*.rs')) + list(root.glob('aida-*/tests/*.rs'))
    for source in sources:
        path = source.relative_to(root).as_posix()
        module = ('ratchet' in source.stem or source.stem.endswith(
            ('_inventory_tests', '_source_scan_tests', '_golden_tests', '_audit_tests')))
        text = source.read_text()
        # Cheap prefilter: a marker-named test implies a marker-named fn.
        if not module and path not in reviewed and not re.search(r'fn\s+\w*(?:ratchet|inventor)\w*', text):
            continue
        names = TEST.findall(_mask.mask_rust(text))
        if module or path in reviewed or any(marker in name for name in names for marker in MARKERS):
            result.update(f'{path}::{name}' for name in names)
    return result


def selected():
    paths = re.findall(r'#\[path = "../(aida-[^"\n]+\.rs)"\]', HARNESS.read_text())
    return {path for path in paths if TEST.search(_mask.mask_rust((ROOT / path).read_text()))}


def missing(root, paths, exclusions, selections=frozenset()):
    return {test for test in candidates(root, selections)
            if test.rsplit('::', 1)[0] not in paths and test not in exclusions
            and test not in selections}


class StructuralSelection(unittest.TestCase):
    def test_selection_is_a_checked_cover(self):
        paths = selected()
        exclusions = json.loads(EXCLUSIONS.read_text())
        selections = ratchet_selections()
        self.assertGreaterEqual(len(paths), 8)
        self.assertTrue(all(reason.strip() for reason in exclusions.values()))
        self.assertEqual(missing(ROOT, paths, exclusions, selections), set(),
                         'Add the source module to aida-ci-guards/lib.rs, select the test in '
                         'scripts/ratchet-set.json, or justify an explicit exclusion')
        self.assertEqual(set(exclusions) - candidates(ROOT, selections), set(), 'Remove stale exclusions')
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

    # trace:STORY-2833 | ai:claude
    def test_unrecognized_test_in_a_mixed_module_requires_a_disposition(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'aida-cli-lib/src/tests/mixed_tests.rs'
            source.parent.mkdir(parents=True)
            source.write_text('#[test]\nfn guard_ratchet() {}\n#[test]\nfn plain_behavior() {}')
            path = source.relative_to(root).as_posix()
            selections = {path + '::guard_ratchet'}
            # The behavioral sibling is not ignored for lacking a marker word.
            self.assertEqual(missing(root, set(), {}, selections), {path + '::plain_behavior'})
            self.assertEqual(missing(root, set(), {path + '::plain_behavior': 'FULL'}, selections), set())
            # A reviewed ratchet-set source is mixed even without marker names.
            source.write_text('#[test]\nfn classified() {}\n#[test]\nfn parses() {}')
            self.assertEqual(missing(root, set(), {}, {path + '::classified'}), {path + '::parses'})

    # trace:STORY-2833 | ai:claude
    def test_new_unlisted_test_in_a_real_mixed_module_fails_the_cover(self):
        path = 'aida-cli-lib/src/tests/bug_2398_codex_tool_env_tests.rs'
        source = ROOT / path
        original = Path.read_text

        def injected(file, *args, **kwargs):
            text = original(file, *args, **kwargs)
            return text + '\n#[test]\nfn new_unlisted_behavior() {}\n' if file == source else text

        with patch.object(Path, 'read_text', injected):
            gaps = missing(ROOT, selected(), json.loads(EXCLUSIONS.read_text()), ratchet_selections())
        self.assertEqual(gaps, {path + '::new_unlisted_behavior'})

    def test_executable_lookup_guard_requires_fast_selection(self):
        # The lib-0 train failure from #3926 must be covered even though its
        # original test name contains neither "ratchet" nor "inventory".
        path = 'aida-cli-lib/src/tests/resolve_aida_exe_ratchet_tests.rs'
        test = path + '::no_cli_source_outside_the_resolver_uses_raw_current_executable_lookup'
        self.assertIn(test, candidates(ROOT))
        self.assertIn(test, missing(ROOT, selected() - {path}, {}))
        self.assertNotIn(test, missing(ROOT, selected(), {}))

    # trace:BUG-2574 | ai:codex
    def test_process_identity_guard_requires_fast_selection(self):
        path = 'aida-cli-lib/src/tests/task_1750_liveness_primitive_guard_tests.rs'
        # The production scan's name has neither "ratchet" nor "inventory".
        # Pin it explicitly so removing/renaming its companion syntax test
        # cannot make the train regression disappear from the selection audit.
        names = TEST.findall(_mask.mask_rust((ROOT / path).read_text()))
        self.assertIn('proc_identity_legacy_decisions_do_not_expand', names)
        self.assertIn(path, selected())


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
