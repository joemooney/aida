"""The aida-json-union merge driver for append-only JSON lists. trace:BUG-2784 | ai:claude"""
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
DRIVER = ROOT / 'scripts/json-union-merge.py'
FILE = 'scripts/ratchet-set.json'
BASE = {'version': 1, 'groups': [{'package': 'p', 'target': ['--lib'], 'filter': 'f::',
                                  'tests': ['alpha', 'beta']}],
        'shared_modules': {'m.rs': ['one']}}


class JsonUnionMerge(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.repo = Path(self.dir.name)
        self.git('init', '-q', '-b', 'main')
        attributes = (ROOT / '.gitattributes').read_text()
        self.assertIn(f'{FILE} merge=aida-json-union', attributes)
        (self.repo / '.gitattributes').write_text(attributes)
        self.write(BASE)
        self.git('add', '-A')
        self.git('commit', '-qm', 'base')

    def git(self, *args):
        driver = f'{shlex.quote(sys.executable)} -I {shlex.quote(str(DRIVER))} %O %A %B'
        return subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@t',
                               '-c', 'core.hooksPath=/dev/null',
                               '-c', f'merge.aida-json-union.driver={driver}', *args],
                              cwd=self.repo, capture_output=True, text=True)

    def write(self, data):
        path = self.repo / FILE
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2) + '\n')

    def branch(self, name, edit):
        self.git('checkout', '-q', '-b', name, 'main')
        data = json.loads(json.dumps(BASE))
        edit(data)
        self.write(data)
        self.git('commit', '-qam', name)

    def merge(self, ours, theirs):
        self.git('checkout', '-q', ours)
        return self.git('merge', '--no-edit', theirs)

    def test_disjoint_additions_to_one_group_merge_cleanly(self):
        def ours(data):
            data['groups'][0]['tests'].append('gamma')
            data['shared_modules']['m.rs'].append('two')
        def theirs(data):
            data['groups'][0]['tests'].append('delta')
            data['groups'].append({'package': 'q', 'target': ['--lib'], 'filter': 'g', 'tests': ['x']})
        self.branch('a', ours)
        self.branch('b', theirs)
        result = self.merge('a', 'b')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        merged = json.loads((self.repo / FILE).read_text())
        # Both sides were sorted, so the union stays sorted.
        self.assertEqual(merged['groups'][0]['tests'], ['alpha', 'beta', 'delta', 'gamma'])
        self.assertEqual([g['filter'] for g in merged['groups']], ['f::', 'g'])
        self.assertEqual(merged['shared_modules']['m.rs'], ['one', 'two'])
        tree = self.git('merge-tree', '--write-tree', 'a', 'b')
        self.assertEqual(tree.returncode, 0, tree.stdout)

    def assert_markers(self, *sides):
        text = (self.repo / FILE).read_text()
        for needle in ('<<<<<<< ours', '=======', '>>>>>>> theirs', *sides):
            self.assertIn(needle, text)

    def assert_conflict(self, ours, theirs, *sides):
        result = self.merge(ours, theirs)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('CONFLICT', result.stdout)
        # A failed driver never leaves a clean-looking OURS file behind.
        self.assert_markers(*sides)
        self.git('merge', '--abort')

    def test_same_key_edited_differently_stays_a_conflict(self):
        self.branch('b', lambda data: data.update(version=2))
        self.branch('c', lambda data: data.update(version=3))
        self.assert_conflict('b', 'c', '"version": 2', '"version": 3')
        # Removing an entry one side still edits is also a real conflict.
        self.branch('d', lambda data: data['groups'][0]['tests'].append('gamma'))
        self.branch('e', lambda data: data['groups'].clear())
        self.assert_conflict('d', 'e', '"gamma"')

    def test_duplicate_entries_conflict_with_markers(self):
        self.branch('f', lambda data: data['shared_modules']['m.rs'].extend(['dup', 'dup']))
        self.branch('g', lambda data: data['shared_modules']['m.rs'].insert(0, 'zeta'))
        self.assert_conflict('f', 'g', '"dup"', '"zeta"')

    def test_invalid_json_conflicts_with_markers(self):
        self.git('checkout', '-q', '-b', 'h', 'main')
        (self.repo / FILE).write_text('{"version": 1,\n')
        self.git('commit', '-qam', 'broken')
        self.branch('i', lambda data: data.update(version=4))
        self.assert_conflict('h', 'i', '{"version": 1,', '"version": 4')

    def test_clean_text_merge_of_a_refused_json_merge_conflicts_whole_files(self):
        # Disjoint hunks text-merge cleanly, but the duplicate is still refused.
        lines = [f'"t{n}"' for n in range(8)]
        texts = {'base': lines, 'ours': ['"t0"'] + lines, 'theirs': lines + ['"t9"']}
        for name, items in texts.items():
            (self.repo / name).write_text('[\n  ' + ',\n  '.join(items) + '\n]\n')
        paths = [str(self.repo / name) for name in texts]
        result = subprocess.run([sys.executable, '-I', str(DRIVER), *paths],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 3, result.stderr)
        text = (self.repo / 'ours').read_text()
        self.assertTrue(text.startswith('<<<<<<< ours\n['), text)
        self.assertIn('=======\n[', text)
        self.assertIn('"t9"', text)
        self.assertTrue(text.endswith('>>>>>>> theirs\n'), text)

if __name__ == '__main__':
    unittest.main()
