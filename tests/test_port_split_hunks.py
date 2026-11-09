"""Host merge fixture coverage. trace:STORY-2692 | ai:codex"""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/port-split-hunks.py'
SPEC = importlib.util.spec_from_file_location('port_split', SCRIPT)
PORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PORT)


class PortSplitHunks(unittest.TestCase):
    def test_exact_unique_and_atomic_refusal(self):
        files = {'a': ['one\n', 'old\n', 'three\n'], 'b': ['other\n']}
        self.assertEqual(PORT.plan([(['old\n'], ['new\n'])], files), {'a': 'one\nnew\nthree\n'})
        for old in (['missing\n'], [], ['other \n']):
            with self.assertRaises(PORT.Refusal):
                PORT.plan([(['old\n'], ['new\n']), (old, ['replacement\n'])], files)
        self.assertEqual(files['a'][1], 'old\n')
        with self.assertRaises(PORT.Refusal):
            PORT.plan([(['old\n'], ['new\n'])], {'a': ['old\n'], 'b': ['old\n']})
        with self.assertRaises(PORT.Refusal):
            PORT.plan([(['old\n'], ['new\n'])], {'a': ['old\n', 'old\n']})
        with self.assertRaises(PORT.Refusal):
            PORT.plan([(['old\n'], ['new\n']), (['old\n'], ['again\n'])], files)

    def test_missing_newline_and_malformed_patch(self):
        patch = '@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n'
        self.assertEqual(PORT.parse_hunks(patch), [(['old'], ['new'])])
        for patch in ('', 'Binary files differ\n', '@@ -1,2 +1 @@\n-a\n+b\n'):
            with self.assertRaises(PORT.Refusal):
                PORT.parse_hunks(patch)

    def test_real_split_merge_with_conflicted_inventory(self):
        # trace:STORY-2692 | ai:codex
        import shutil
        import tomllib
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(['git', '-C', str(root), *args],
                                      capture_output=True, text=True, check=True).stdout.strip()
            def commit():
                git('add', '.')
                git('commit', '-qm', 'fixture')
                return git('rev-parse', 'HEAD')
            git('init', '-q')
            git('config', 'user.name', 'Fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            scripts = root / 'scripts'
            scripts.mkdir()
            shutil.copy(SCRIPT.parent / 'check-store-writers.py', scripts)
            inventory = scripts / 'store-writers.toml'
            inventory.write_text('version = 2\nwriter = []\noperation = []\n# base\n')
            src = root / 'aida-core/src'
            src.mkdir(parents=True)
            first = ''.join(f'// first {i}\n' for i in range(8)) + 'pub fn first() {}\n' + ''.join(f'// first tail {i}\n' for i in range(8))
            second = ''.join(f'// second {i}\n' for i in range(8)) + 'pub fn moved() {}\n' + ''.join(f'// tail {i}\n' for i in range(8))
            source = src / 'source.rs'
            (root / 'README').write_text('older base\n')
            older = commit()
            source.write_text(first + second)
            base = commit()
            git('checkout', '-qb', 'pr')
            changed = second.replace('moved() {}', 'moved() { /* PR edit */ }')
            source.write_text(first.replace('first() {}', 'first() { /* PR */ }') + changed)
            inventory.write_text(inventory.read_text().replace('# base', '# PR'))
            head = commit()
            git('checkout', '-qb', 'split', base)
            source.write_text(first + 'mod moved;\npub use moved::moved;\n')
            (src / 'first.rs').write_text(first)
            module = src / 'moved.rs'
            module.write_text(second)
            inventory.write_text(inventory.read_text().replace('# base', '# split'))
            commit()
            baseline = root / 'reviewed.toml'
            baseline.write_text(inventory.read_text())
            merge = subprocess.run(['git', '-C', str(root), 'merge', '--no-commit', 'pr'], capture_output=True)
            self.assertEqual(merge.returncode, 1)
            self.assertIn('<<<<<<<', source.read_text())
            self.assertIn('<<<<<<<', inventory.read_text())
            command = [sys.executable, str(SCRIPT), '--root', str(root),
                       '--base', base, '--head', head, '--source', 'aida-core/src/source.rs',
                       '--destination', 'aida-core/src/moved.rs',
                       '--destination', 'aida-core/src/source.rs',
                       '--destination-conflict-side', 'ours', '--apply',
                       '--inventory-baseline', str(baseline)]
            # Bad policy evidence refuses before any hunk writes.
            baseline.write_text('<<<<<<< invalid\n')
            refused = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertEqual(module.read_text(), second)
            baseline.write_text('version = 2\nwriter = []\noperation = []\n')
            # A wrong ancestor must not replay unrelated changes onto main.
            wrong = subprocess.run([*command, '--base', older], capture_output=True, text=True)
            self.assertNotEqual(wrong.returncode, 0)
            self.assertIn('unique merge base', wrong.stderr)
            self.assertEqual(module.read_text(), second)
            implicit = command[:]
            side = implicit.index('--destination-conflict-side')
            del implicit[side:side + 2]
            refused = subprocess.run(implicit, capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('select --destination-conflict-side', refused.stderr)
            self.assertEqual(module.read_text(), second)
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(module.read_text(), changed)
            self.assertIn('deferred', result.stdout)
            self.assertIn('<<<<<<<', inventory.read_text())
            self.assertEqual(source.read_text(), first.replace('first() {}', 'first() { /* PR */ }') + 'mod moved;\npub use moved::moved;\n')
            git('add', 'aida-core/src/source.rs')
            regenerated = subprocess.run([sys.executable, str(scripts / 'check-store-writers.py'), '--root', str(root), '--regenerate', '--baseline', str(baseline)], capture_output=True, text=True)
            self.assertEqual(regenerated.returncode, 0, regenerated.stderr)
            self.assertEqual(tomllib.loads(inventory.read_text())['version'], 2)
            self.assertNotIn('<<<<<<<', inventory.read_text())
            git('add', '.')
            git('commit', '-qm', 'resolve split merge')
            self.assertEqual(len(git('rev-list', '--parents', '-n', '1', 'HEAD').split()), 3)

    def test_seven_prs_and_reexport_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.STDOUT).decode().strip()
            def commit():
                git('add', '.')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
                return git('rev-parse', 'HEAD')
            git('init', '-q')
            original = ''.join(f'// context {i}\n' for i in range(8)) + 'pub fn moved() {}\n' + ''.join(f'// tail {i}\n' for i in range(8))
            (root / 'source.rs').write_text(original)
            base = commit()
            (root / 'module.rs').write_text(original)
            (root / 'scripts').mkdir()
            # Stub checks the integration argv; the real inventory scanner has its own tests.
            (root / 'scripts/check-store-writers.py').write_text("import pathlib, sys\nassert sys.argv[1:] == ['--root', str(pathlib.Path.cwd()), '--regenerate']\npathlib.Path('regenerated').write_text('yes')\n")
            def run(head, *extra):
                return subprocess.run([sys.executable, str(SCRIPT), '--root', str(root), '--base', base, '--head', head, '--source', 'source.rs', '--destination', 'module.rs', *extra], cwd=root, capture_output=True, text=True)
            for number in range(7):
                changed = original.replace('moved() {}', f'moved() {{ /* PR {number} */ }}')
                (root / 'source.rs').write_text(changed)
                head = commit()
                (root / 'module.rs').write_text(original)
                result = run(head)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((root / 'module.rs').read_text(), original)
                result = run(head, '--apply')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((root / 'module.rs').read_text(), changed)
                self.assertTrue((root / 'regenerated').exists())
            (root / 'module.rs').write_text(original)
            (root / 'scripts/check-store-writers.py').write_text('raise SystemExit(7)\n')
            result = run(head, '--apply')
            self.assertEqual(result.returncode, 7)
            self.assertEqual((root / 'module.rs').read_text(), changed)
            # PR edits a parent-facing export as well as the moved implementation.
            # Its missing parent context must not trigger a guessed re-export.
            (root / 'source.rs').write_text(original + '\npub use moved as public_api;\n')
            head = commit()
            (root / 'module.rs').write_text('pub fn moved() {}\n')
            (root / 'regenerated').unlink()
            result = run(head, '--apply')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('re-exports', result.stderr)
            self.assertEqual((root / 'module.rs').read_text(), 'pub fn moved() {}\n')
            self.assertFalse((root / 'regenerated').exists())
            (root / 'link.rs').symlink_to(root / 'module.rs')
            result = run(head, '--destination', 'link.rs', '--apply')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('symlink', result.stderr)


if __name__ == '__main__':
    unittest.main()
