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
