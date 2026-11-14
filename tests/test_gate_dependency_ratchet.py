"""Gate command-layer dependency ratchet regression fixtures."""
# trace:TASK-3785 | ai:codex
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('gate_ratchet', ROOT / 'scripts/check-gate-dependencies.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def ledger(edges):
    return {'version': 1, 'owner': 'TASK-3785', 'ceiling': sum(map(len, edges.values())), 'edges': edges}


class GateDependencies(unittest.TestCase):
    def test_aliases_grouped_imports_references_and_deduplication(self):
        source = '''
        use crate::{pr_cmd::{self as pr, ship as send}, session as sess};
        use sess::launch as start;
        fn a() { pr::ship(); send(); start(); sess::launch(); }
        fn b() { let callback = crate::pr_cmd::ship; }
        '''
        self.assertEqual(gate.edges(source), {'crate::pr_cmd::ship', 'crate::session::launch'})

    def test_literals_comments_and_other_layers_are_ignored(self):
        self.assertEqual(gate.edges('''
        // crate::session::fake();
        /* outer /* crate::pr_cmd::fake */ comment */
        const S: &str = r###"crate::merge_hold::fake()"###;
        fn a() { crate::gate::ship::check(); crate::forge::read(); }
        '''), set())

    def test_ancestor_paths_raw_identifiers_types_and_macros(self):
        self.assertEqual(gate.edges('''
        use super::super::pr_cmd as pr;
        fn a(x: crate::auto_complete::PhaseFailure) {
            pr::r#ship(); crate::pr_train::mark!();
            super::super::session::launch();
        }
        '''), {'crate::pr_cmd::ship', 'crate::auto_complete::PhaseFailure',
               'crate::pr_train::mark', 'crate::session::launch'})

    def test_tests_are_counted_but_enum_variants_are_one_item(self):
        self.assertEqual(gate.edges('''
        #[cfg(test)] mod tests { fn a() {
            crate::pr_ship::ShipStep::Merge; crate::pr_ship::ShipStep::Push;
        }}
        '''), {'crate::pr_ship::ShipStep'})

    def test_unresolved_ancestor_module_aliases_refuse(self):
        # trace:TASK-3785 | ai:codex
        for imported in ('super::super as root',
                         'super::super::{self as root}',
                         'super::{super::{self as root}}',
                         'super::{self as root}'):
            with self.subTest(imported=imported), self.assertRaisesRegex(
                    ValueError, 'unresolved ancestor-module path'):
                gate.edges(f'use {imported}; fn f() {{ root::session::new_item(); }}')

    def test_ancestor_visibility_is_not_an_import_binding(self):
        self.assertEqual(gate.edges('pub(super) fn f() {}'), set())

    def test_globs_cannot_hide_items(self):
        for source in ('use crate::*;', 'use crate::session::*;',
                       'use crate::pr_cmd as p; use p::*;'):
            with self.subTest(source=source), self.assertRaises(ValueError):
                gate.edges(source)

    def test_growth_refuses_raised_ceiling_and_replacement(self):
        old = ledger({'a.rs': ['crate::pr_cmd::a', 'crate::pr_cmd::b']})
        gate.check_growth(ledger({'b.rs': ['crate::pr_cmd::a']}), old)
        for new in (ledger({'a.rs': ['crate::pr_cmd::a', 'crate::pr_cmd::b', 'crate::pr_cmd::c']}),
                    ledger({'a.rs': ['crate::session::c']})):
            with self.assertRaises(ValueError):
                gate.check_growth(new, old)
        bad = ledger({'a.rs': ['crate::pr_cmd::a']})
        bad['ceiling'] += 1
        with self.assertRaises(ValueError):
            gate.validate(bad)

    def test_new_unstaged_gate_files_are_scanned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'aida-cli-lib/src/gate/new.rs'
            path.parent.mkdir(parents=True)
            path.write_text('fn a() { crate::session::launch(); }')
            self.assertEqual(gate.scan(root), {str(path.relative_to(root)): ['crate::session::launch']})

    def test_base_bootstrap_shrink_and_growth(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', *args], cwd=root)
            git('init', '-q')
            path = root / 'aida-cli-lib/src/gate/new.rs'
            path.parent.mkdir(parents=True)
            path.write_text('fn a() { crate::session::launch(); }')
            (root / 'scripts').mkdir()
            git('add', '.')
            git('-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'base')
            def run(base='HEAD'):
                with mock.patch.object(gate, 'ROOT', root), mock.patch.object(gate, 'scan', lambda: gate.scan_original(root)), mock.patch('sys.argv', ['ratchet', '--write-ledger', '--base', base]):
                    gate.main()
            with mock.patch.object(gate, 'scan_original', gate.scan, create=True):
                run()  # Initial introduction, no ledger in base.
                git('add', '.')
                git('-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'ledger')
                saved = (root / gate.LEDGER).read_text()
                # An alias added to an existing file must refuse with the old
                # ledger, including --base checks and attempted ledger writes.
                for imported in ('super::super as root',
                                 'super::super::{self as root}',
                                 'super::{super::{self as root}}'):
                    path.write_text('fn a() { crate::session::launch(); }\n'
                                    f'use {imported}; fn f() {{ root::session::new_item(); }}')
                    for write in (False, True):
                        args = ['ratchet', '--base', 'HEAD'] + (['--write-ledger'] if write else [])
                        with self.subTest(imported=imported, write=write), \
                                mock.patch.object(gate, 'ROOT', root), \
                                mock.patch.object(gate, 'scan', lambda: gate.scan_original(root)), \
                                mock.patch('sys.argv', args), \
                                self.assertRaisesRegex(ValueError, 'unresolved ancestor-module path'):
                            gate.main()
                        self.assertEqual((root / gate.LEDGER).read_text(), saved)
                path.write_text('fn a() { crate::session::launch(); crate::session::extra(); }')
                with self.assertRaises(ValueError):
                    run()
                self.assertEqual((root / gate.LEDGER).read_text(), saved)
                path.write_text('fn a() {}')
                run()
                self.assertEqual(json.loads((root / gate.LEDGER).read_text())['ceiling'], 0)
                with self.assertRaises(subprocess.CalledProcessError):
                    run('missing-ref')
