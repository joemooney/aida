"""Crate split guard and pure-move ledger fixtures."""
# trace:TASK-3428 | ai:codex
import importlib.util
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GLOB = load('check-crate-globs')
MOVE = load('relocate-ledger-paths')


class CrateSplitTools(unittest.TestCase):
    def git(self, root, *args):
        return subprocess.run(
            ['git', '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', *args],
            cwd=root, check=True, capture_output=True, text=True)

    def repository(self, ledger=True):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        self.git(root, 'init', '-q')
        (root / 'old.rs').write_text('use crate::*;\nuse crate::*;\n')
        (root / 'scripts').mkdir()
        (root / 'scripts/portability-allowlist.txt').write_text('r\told.rs:source\n')
        (root / 'scripts/exec-context-legacy.json').write_text(json.dumps({'sites': {'old.rs:f:var:abc': 2}}))
        (root / 'scripts/ownership-liveness-legacy.json').write_text(
            json.dumps({'old.rs:example:pid_is_alive': 2}))
        if ledger:
            self.write_globs(root, {'old.rs': 2})
        self.git(root, 'add', '.')
        self.git(root, '-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'base')
        return root

    def write_globs(self, root, values):
        (root / GLOB.LEDGER).write_text(json.dumps(values))

    def run_globs(self, root, base='HEAD'):
        with mock.patch.object(GLOB, 'ROOT', root), mock.patch('sys.argv', ['check-crate-globs.py', '--base', base]):
            with contextlib.redirect_stdout(io.StringIO()):
                GLOB.main()

    def test_base_allows_shrink(self):
        root = self.repository()
        (root / 'old.rs').write_text('use crate::*;\n')
        self.write_globs(root, {'old.rs': 1})
        self.run_globs(root)

    def test_base_refuses_growth(self):
        root = self.repository()
        (root / 'old.rs').write_text('use crate::*;\n' * 3)
        self.write_globs(root, {'old.rs': 3})
        with self.assertRaisesRegex(ValueError, 'may only shrink'):
            self.run_globs(root)

    def test_base_refuses_same_count_swap_without_a_rename(self):
        root = self.repository()
        (root / 'old.rs').write_text('fn retired() {}\n')
        (root / 'new.rs').write_text('use crate::*;\n' * 2)
        self.git(root, 'add', '-A')
        self.write_globs(root, {'new.rs': 2})
        with self.assertRaisesRegex(ValueError, 'may only shrink'):
            self.run_globs(root)

    def test_base_refuses_borrowing_from_retained_path(self):
        root = self.repository()
        (root / 'old.rs').write_text('use crate::*;\n')
        (root / 'new.rs').write_text('use crate::*;\n')
        self.git(root, 'add', '-A')
        self.write_globs(root, {'old.rs': 1, 'new.rs': 1})
        with self.assertRaisesRegex(ValueError, 'may only shrink'):
            self.run_globs(root)

    def test_relocated_portability_allowlist_passes_but_growth_refuses(self):
        root = self.repository()
        (root / 'old.rs').rename(root / 'new.rs')
        MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
        base = root / 'base-allowlist'
        base.write_text('r\told.rs:source\n')
        rules = root / 'rules.json'
        rules.write_text('[{"id": "r"}]')
        current = root / 'scripts/portability-allowlist.txt'

        def run():
            return subprocess.run(
                ['python3', str(ROOT / 'scripts/check-portability-growth.py'),
                 str(base), str(rules), str(current), str(rules)],
                capture_output=True, text=True)

        self.assertEqual(run().returncode, 0)
        # One retired row cannot fund two copies of the same content.
        current.write_text(current.read_text() + 'r\tcopy.rs:source\n')
        result = run()
        self.assertEqual(result.returncode, 1)
        self.assertIn('rules are unchanged', result.stderr)

    def test_base_allows_bootstrap(self):
        root = self.repository(ledger=False)
        self.write_globs(root, {'old.rs': 2})
        self.run_globs(root)

    def test_base_allows_pure_relocation_but_refuses_growth_after_move(self):
        root = self.repository()
        (root / 'old.rs').rename(root / 'new.rs')
        self.git(root, 'add', '-A')
        MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
        self.run_globs(root)
        (root / 'new.rs').write_text('use crate::*;\n' * 3)
        self.write_globs(root, {'new.rs': 3})
        with self.assertRaisesRegex(ValueError, 'may only shrink'):
            self.run_globs(root)

    def test_base_read_failure_does_not_bootstrap_existing_ledger(self):
        root = self.repository()
        run = subprocess.run

        def unreadable_show(args, **kwargs):
            if args[:2] == ['git', 'show']:
                return subprocess.CompletedProcess(args, 1, '', 'fixture read failure')
            return run(args, **kwargs)

        with mock.patch.object(GLOB.subprocess, 'run', side_effect=unreadable_show):
            with self.assertRaisesRegex(ValueError, 'cannot read base glob ledger'):
                self.run_globs(root)

    def test_invalid_base_does_not_bootstrap(self):
        root = self.repository()
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_globs(root, 'missing-base-ref')

    def test_malformed_base_ledger_refuses(self):
        root = self.repository()
        (root / GLOB.LEDGER).write_text('invalid json')
        self.git(root, 'add', GLOB.LEDGER)
        self.git(root, '-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'malformed ledger')
        self.write_globs(root, {'old.rs': 2})
        with self.assertRaises(json.JSONDecodeError):
            self.run_globs(root)

    def test_stale_path_in_each_ledger_refuses_before_any_write(self):
        for ledger in ('portability-allowlist.txt', 'exec-context-legacy.json', 'crate-globs-legacy.json', 'ownership-liveness-legacy.json'):
            with self.subTest(ledger=ledger):
                root = self.repository()
                (root / 'old.rs').rename(root / 'new.rs')
                before = {path: path.read_text() for path in (root / 'scripts').iterdir()}
                stale = before[root / 'scripts' / ledger]
                rewrite = MOVE.rewrite

                def incomplete_rewrite(text, *args):
                    return text if text == stale else rewrite(text, *args)

                with mock.patch.object(MOVE, 'rewrite', side_effect=incomplete_rewrite):
                    with self.assertRaisesRegex(ValueError, 'stale path remains'):
                        MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
                self.assertEqual({path: path.read_text() for path in before}, before)

    def test_glob_syntax_and_literals(self):
        self.assertEqual(GLOB.count('use crate::*; use crate::{foo, *}; use ::crate :: *;'), 3)
        self.assertEqual(GLOB.count('/* use crate::*; */ const S: &str = r#"use crate::*;"#; use crate::child::*; use super::*;'), 0)

    def test_additions_and_retirements_require_baseline_update(self):
        for actual in ({'a.rs': 2}, {}):
            with self.assertRaises(ValueError):
                GLOB.check(actual, {'a.rs': 1})
        GLOB.check({'a.rs': 1}, {'a.rs': 1})

    def test_move_verifies_content_and_preserves_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', directory], check=True)
            (root / 'old.rs').write_text('fn example() {}\n')
            subprocess.run(['git', 'add', 'old.rs'], cwd=root, check=True)
            subprocess.run(['git', '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', '-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'fixture'], cwd=root, check=True)
            (root / 'scripts').mkdir()
            (root / 'scripts/portability-allowlist.txt').write_text('# old.rs:comment\nr\told.rs:source old.rs:text\n')
            evidence = {'ceiling': 2, 'sites': {'old.rs:f:var:abc': 2}}
            (root / 'scripts/exec-context-legacy.json').write_text(json.dumps(evidence))
            (root / 'scripts/crate-globs-legacy.json').write_text('{"old.rs": 1}')
            ownership = {'old.rs:example:pid_is_alive': 2, 'other.rs:f:lease_is_live': 1}
            ownership_path = root / 'scripts/ownership-liveness-legacy.json'
            ownership_path.write_text(json.dumps(ownership))
            (root / 'old.rs').rename(root / 'new.rs')
            (root / 'new.rs').write_text('changed')
            with self.assertRaises(ValueError):
                MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
            self.assertEqual(json.loads((root / 'scripts/exec-context-legacy.json').read_text()), evidence)
            self.assertEqual(json.loads(ownership_path.read_text()), ownership)
            (root / 'new.rs').write_text('fn example() {}\n')
            MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
            self.assertEqual(json.loads((root / 'scripts/exec-context-legacy.json').read_text()), {'ceiling': 2, 'sites': {'new.rs:f:var:abc': 2}})
            self.assertEqual((root / 'scripts/portability-allowlist.txt').read_text(), '# old.rs:comment\nr\tnew.rs:source old.rs:text\n')
            self.assertEqual(json.loads((root / 'scripts/crate-globs-legacy.json').read_text()), {'new.rs': 1})
            self.assertEqual(json.loads(ownership_path.read_text()),
                             {'new.rs:example:pid_is_alive': 2, 'other.rs:f:lease_is_live': 1})

    def test_collision_refuses(self):
        with self.assertRaises(ValueError):
            MOVE.rewrite('{"old.rs": 1, "new.rs": 1}', 'old.rs', 'new.rs', True)


if __name__ == '__main__':
    unittest.main()
