"""Crate split guard and pure-move ledger fixtures."""
# trace:TASK-3428 | ai:codex
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GLOB = load('check-crate-globs')
MOVE = load('relocate-ledger-paths')


class CrateSplitTools(unittest.TestCase):
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
            subprocess.run(['git', '-c', 'user.name=fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'fixture'], cwd=root, check=True)
            (root / 'scripts').mkdir()
            (root / 'scripts/portability-allowlist.txt').write_text('# old.rs:comment\nr\told.rs:source old.rs:text\n')
            evidence = {'ceiling': 2, 'sites': {'old.rs:f:var:abc': 2}}
            (root / 'scripts/exec-context-legacy.json').write_text(json.dumps(evidence))
            (root / 'scripts/crate-globs-legacy.json').write_text('{"old.rs": 1}')
            (root / 'old.rs').rename(root / 'new.rs')
            (root / 'new.rs').write_text('changed')
            with self.assertRaises(ValueError):
                MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
            self.assertEqual(json.loads((root / 'scripts/exec-context-legacy.json').read_text()), evidence)
            (root / 'new.rs').write_text('fn example() {}\n')
            MOVE.relocate(root, 'old.rs', 'new.rs', 'HEAD')
            self.assertEqual(json.loads((root / 'scripts/exec-context-legacy.json').read_text()), {'ceiling': 2, 'sites': {'new.rs:f:var:abc': 2}})
            self.assertEqual((root / 'scripts/portability-allowlist.txt').read_text(), '# old.rs:comment\nr\tnew.rs:source old.rs:text\n')
            self.assertEqual(json.loads((root / 'scripts/crate-globs-legacy.json').read_text()), {'new.rs': 1})

    def test_collision_refuses(self):
        with self.assertRaises(ValueError):
            MOVE.rewrite('{"old.rs": 1, "new.rs": 1}', 'old.rs', 'new.rs', True)


if __name__ == '__main__':
    unittest.main()
