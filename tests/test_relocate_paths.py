"""Split relocations conserve budgets and refuse before rewriting any ledger."""
# trace:TASK-3426 | ai:codex
import importlib.util
import json
from pathlib import Path
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('relocate_paths', ROOT / 'scripts/relocate-paths.py')
MOVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MOVE)


class SplitRelocationTests(unittest.TestCase):
    def fixture(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        old = 'aida-cli-lib/src/old.rs'
        destinations = [f'aida-cli-lib/src/old/{name}.rs' for name in ('list', 'next')]
        for name in destinations:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fn arm() { std::env::var("EXAMPLE"); lease_is_live(); }\n')
        (root / 'scripts').mkdir()
        env = MOVE.load('check-exec-context')
        site = env.rust_sites((root / destinations[0]).read_text())[0]
        self.put(root, 'scripts/exec-context-legacy.json', {'ceiling': 2, 'sites': {old + ':' + site: 2}})
        self.put(root, 'scripts/ownership-liveness-legacy.json', {old + ':arm:lease_is_live': 2})
        self.put(root, 'scripts/crate-globs-legacy.json', {})
        vendor = root / 'aida-cli-lib/src/tests/story_2539_vendor_literals.tsv'
        vendor.parent.mkdir(parents=True)
        vendor.write_text('')
        (root / 'scripts/store-writers.toml').write_text('version = 2\nwriter = []\noperation = []\n')
        return root, old, destinations, site

    def put(self, root, name, value):
        (root / name).write_text(json.dumps(value))

    def test_split_keeps_exact_expression_counts_and_ceiling(self):
        root, old, destinations, site = self.fixture()
        MOVE.relocate(root, old, destinations)
        env = json.loads((root / 'scripts/exec-context-legacy.json').read_text())
        self.assertEqual(env, {'ceiling': 2, 'sites': {p + ':' + site: 1 for p in destinations}})
        live = json.loads((root / 'scripts/ownership-liveness-legacy.json').read_text())
        self.assertEqual(live, {p + ':arm:lease_is_live': 1 for p in destinations})

    def test_missing_expression_refuses_without_partial_ledger_updates(self):
        root, old, destinations, site = self.fixture()
        self.put(root, 'scripts/ownership-liveness-legacy.json', {old + ':arm:lease_is_live': 3})
        paths = [p for p in root.rglob('*') if p.is_file()]
        before = {p: p.read_bytes() for p in paths}
        with self.assertRaisesRegex(ValueError, 'non-conserving'):
            MOVE.relocate(root, old, destinations)
        self.assertEqual(before, {p: p.read_bytes() for p in paths})

    def test_config_reader_split_preserves_channels_and_site_total(self):
        root, old, destinations, _ = self.fixture()
        for name in destinations:
            path = root / name
            path.write_text(path.read_text().replace('lease_is_live();',
                            'lease_is_live(); let _ = ".aida/config.toml";'))
        config = root / 'scripts/config-trust.toml'
        config.write_text(f'[readers."{old}"]\nchannels = ["worktree", "prose"]\nsites = 2\n')
        MOVE.relocate(root, old, destinations)
        readers = tomllib.loads(config.read_text())['readers']
        self.assertEqual(readers, {name: {'channels': ['worktree', 'prose'], 'sites': 1}
                                   for name in destinations})

    def test_unhandled_ledgers_refuse_without_any_writes(self):
        for ledger in (
            'scripts/portability-allowlist.txt',
            'scripts/doc-path-exceptions.json',
            'scripts/git-maintenance-baseline.txt',
            'scripts/ci-gate-tiers.toml',
            'aida-cli-lib/src/tests/fixtures/proc_identity_legacy_baseline.json',
        ):
            with self.subTest(ledger=ledger):
                root, old, destinations, _ = self.fixture()
                path = root / ledger
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(old)
                before = {p: p.read_bytes() for p in root.rglob('*') if p.is_file()}
                with self.assertRaisesRegex(ValueError, 'manual relocation required'):
                    MOVE.relocate(root, old, destinations)
                self.assertEqual(before, {p: p.read_bytes() for p in before})

    def test_retained_source_and_escaping_destination_refuse(self):
        root, old, destinations, _ = self.fixture()
        (root / old).write_text('fn arm() {}')
        with self.assertRaisesRegex(ValueError, 'source must have been moved'):
            MOVE.relocate(root, old, destinations)
        with self.assertRaisesRegex(ValueError, 'inside the repository'):
            MOVE.relocate(root, old, ['../escape.rs'])


if __name__ == '__main__':
    unittest.main()
