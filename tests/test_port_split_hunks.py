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

    def test_replay_and_baseline_refusals(self):
        # An EOF insertion leaves its old context intact; it must not duplicate.
        with self.assertRaisesRegex(PORT.Refusal, 'possible replay'):
            PORT.plan([(['tail\n'], ['tail\n', 'added\n'])],
                      {'module': ['tail\n', 'added\n']})
        for data in ({'version': 2}, {'version': 2, 'writer': []},
                     {'version': 2, 'writer': {}},
                     {'version': 2, 'writer': [{'item': 'broken'}]}):
            with self.assertRaises(PORT.Refusal):
                PORT.validate_inventory(data)

    def test_policy_validators_reached_with_populated_writer(self):
        row = dict(id='x.rs|persist|save', path='x.rs', item='persist', sink='save',
                   lock='reviewed', gate='reviewed', disposition='blocked',
                   blocker='review needed', proposed_operation='none', evidence_tests=['fixture'])
        for key, value, message in (
                ('operation', ['broken'], 'incomplete inventory operation'),
                ('mediated_route', 'broken', 'mediated_route must be')):
            with self.subTest(key=key):
                with self.assertRaisesRegex(PORT.Refusal, message):
                    PORT.validate_inventory(dict(version=2, writer=[row], **{key: value}))

    def test_git_line_boundaries(self):
        for separator in ('\v', '\f', '\x85', '\u2028', '\u2029', '\r'):
            text = f'foo{separator}bar\n'
            self.assertEqual(PORT.git_lines(text), [text])
            with self.assertRaises(PORT.Refusal):
                PORT.plan([(['bar\n'], ['new\n'])], {'module': PORT.git_lines(text)})
            patch = f'@@ -1 +1 @@\n-{text}+replacement\n'
            self.assertEqual(PORT.parse_hunks(patch), [([text], ['replacement\n'])])

    def test_merged_main_base_and_nonancestor_refusals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.STDOUT).decode().strip()
            def commit():
                git('add', '.')
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
                return git('rev-parse', 'HEAD')
            git('init', '-q')
            # Even --no-commit merges require a committer identity. Keep this
            # fixture independent of the developer's global Git configuration.
            git('config', 'user.name', 'Fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            (root / 'source.rs').write_text('old\n')
            original = commit()
            git('checkout', '-qb', 'mainline')
            (root / 'main-only').write_text('main update\n')
            updated = commit()
            git('checkout', '-qb', 'pr', original)
            (root / 'source.rs').write_text('new\n')
            pr = commit()
            command = [sys.executable, str(SCRIPT), '--root', str(root), '--source', 'source.rs', '--destination', 'module.rs']
            (root / 'module.rs').write_text('old\n')
            refused = subprocess.run([*command, '--base', updated, '--head', pr], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('must be an ancestor', refused.stderr)
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'merge', '--no-edit', 'mainline')
            head = git('rev-parse', 'HEAD')
            git('checkout', 'mainline')
            # Simulate the active merge before committing its clean result.
            git('merge', '--no-commit', '--no-ff', 'pr')
            refused = subprocess.run([*command, '--base', original, '--head', head], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('unique merge base', refused.stderr)
            valid = subprocess.run([*command, '--base', updated, '--head', head], capture_output=True, text=True)
            self.assertEqual(valid.returncode, 0, valid.stderr)
            # Missing regenerator refuses before source edits.
            refused = subprocess.run([*command, '--base', updated, '--head', head, '--apply'], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('existing regular file', refused.stderr)
            self.assertEqual((root / 'module.rs').read_text(), 'old\n')

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
            # A populated reviewed policy row survives regeneration.
            writer = src / 'writer.rs'
            writer.write_text('pub fn persist() { std::fs::write("file", "body").unwrap(); }\n')
            spec = importlib.util.spec_from_file_location('guard', SCRIPT.parent / 'check-store-writers.py')
            guard = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(guard)
            guard.regenerate(root)
            inventory.write_text(inventory.read_text().replace('caller-owned; inspect enclosing item before cutover', 'reviewed fixture lock'))
            inventory.write_text(inventory.read_text() + '# base\n')
            reviewed = inventory.read_text()
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
            split_tip = commit()
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
            baseline.write_text('version = 2\nwriter = [{item = "broken"}]\n')
            refused = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('incomplete inventory writer', refused.stderr)
            self.assertEqual(module.read_text(), second)
            baseline.write_text(reviewed)
            for empty in ('version = 2\n', 'version = 2\nwriter = []\n'):
                baseline.write_text(empty)
                refused = subprocess.run(command, capture_output=True, text=True)
                self.assertNotEqual(refused.returncode, 0)
                self.assertIn('empty policy refused', refused.stderr)
                self.assertEqual(module.read_text(), second)
            baseline.write_text(reviewed)
            before_refusals = {path: path.read_bytes() for path in (source, module, inventory)}
            wrong_head = subprocess.run([*command, '--head', base], capture_output=True, text=True)
            self.assertNotEqual(wrong_head.returncode, 0)
            self.assertIn('--head must be', wrong_head.stderr)
            self.assertEqual(module.read_text(), second)
            # A wrong ancestor must not replay unrelated changes onto main.
            wrong = subprocess.run([*command, '--base', older], capture_output=True, text=True)
            self.assertNotEqual(wrong.returncode, 0)
            self.assertIn('unique merge base', wrong.stderr)
            for path, content in before_refusals.items():
                self.assertEqual(path.read_bytes(), content)
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
            self.assertIn('reviewed fixture lock', inventory.read_text())
            git('add', '.')
            git('commit', '-qm', 'resolve split merge')
            self.assertEqual(len(git('rev-list', '--parents', '-n', '1', 'HEAD').split()), 3)
            # Repository workflow: merge the split main branch INTO the PR.
            git('checkout', 'pr')
            baseline.write_text(reviewed)
            merge = subprocess.run(['git', '-C', str(root), 'merge', '--no-commit', split_tip], capture_output=True)
            self.assertEqual(merge.returncode, 1)
            reverse = command[:]
            reverse[reverse.index('ours')] = 'theirs'
            result = subprocess.run(reverse, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(module.read_text(), changed)
            self.assertIn('/* PR */', source.read_text())


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
