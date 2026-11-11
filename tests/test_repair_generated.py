"""Real Git fixtures for the host-embedded regeneration driver.
trace:STORY-2710 | ai:codex
"""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

DRIVER = Path(__file__).resolve().parents[1] / 'scripts/repair-generated.py'
spec = importlib.util.spec_from_file_location('repair_generated', DRIVER)
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class RepairGenerationTests(unittest.TestCase):
    def fixture(self, root, extra=''):
        def git(*args, check=True):
            return subprocess.run(('git', *args), cwd=root, check=check, capture_output=True, text=True)
        git('init', '-q', '-b', 'main')
        git('config', 'user.name', 'Test')
        git('config', 'user.email', 'test@example.invalid')
        (root / 'scripts').mkdir()
        (root / 'scripts/exec-context-legacy.json').write_text('base\n')
        (root / 'source.rs').write_text('base\n')
        (root / 'scripts/check-exec-context.py').write_text(
            'from pathlib import Path\nimport sys\n'
            'if "--write-ledger" in sys.argv:\n'
            '    Path("scripts/exec-context-legacy.json").write_text("regenerated\\n")\n' + extra)
        git('add', '.')
        git('commit', '-qm', 'base')
        git('checkout', '-qb', 'pr')
        (root / 'scripts/exec-context-legacy.json').write_text('pr\n')
        git('commit', '-qam', 'pr')
        git('checkout', 'main')
        (root / 'scripts/exec-context-legacy.json').write_text('main\n')
        git('commit', '-qam', 'main')
        base = git('rev-parse', 'HEAD').stdout.strip()
        git('checkout', 'pr')
        self.assertEqual(git('merge', '--no-edit', 'main', check=False).returncode, 1)
        return git, base

    def run_driver(self, root, base, phase):
        return subprocess.run(('python3', str(DRIVER), base, phase), cwd=root, capture_output=True, text=True)

    def test_restore_and_regenerate_conflicted_ledger_retains_merge(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, base = self.fixture(root)
            self.assertEqual(self.run_driver(root, base, 'restore').returncode, 0)
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'main\n')
            self.assertEqual(self.run_driver(root, base, 'generate').returncode, 0)
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'regenerated\n')
            self.assertEqual(git('rev-parse', 'MERGE_HEAD').stdout.strip(), base)
            self.assertEqual(git('diff', '--name-only', '--diff-filter=U').stdout, '')

    def test_unexpected_generator_write_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, base = self.fixture(root, '    Path("source.rs").write_text("unexpected")\n')
            self.assertEqual(self.run_driver(root, base, 'restore').returncode, 0)
            result = self.run_driver(root, base, 'generate')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('unregistered outputs', result.stderr)

    def test_unresolved_authoritative_source_allows_restoration_but_blocks_generation(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, base = self.fixture(root)
            # Add an unmerged source entry using a real three-stage index.
            oid = git('rev-parse', 'HEAD:source.rs').stdout.strip()
            subprocess.run(('git', 'update-index', '--index-info'), cwd=root,
                           input=f'0 {"0" * 40}\tsource.rs\n100644 {oid} 1\tsource.rs\n100644 {oid} 2\tsource.rs\n100644 {oid} 3\tsource.rs\n',
                           text=True, check=True, capture_output=True)
            before = (root / 'scripts/exec-context-legacy.json').read_bytes()
            result = self.run_driver(root, base, 'restore')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotEqual((root / 'scripts/exec-context-legacy.json').read_bytes(), before)
            result = self.run_driver(root, base, 'generate')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('authoritative sources', result.stderr)
            git('add', 'source.rs')
            result = self.run_driver(root, base, 'generate')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(git('rev-parse', 'MERGE_HEAD').stdout.strip(), base)

    def test_real_source_conflict_with_registered_generator_reaches_resolution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, _ = self.fixture(root)
            git('merge', '--abort')
            (root / 'source.rs').write_text('pr source\n')
            git('commit', '-qam', 'PR source change')
            git('checkout', 'main')
            (root / 'source.rs').write_text('main source\n')
            git('commit', '-qam', 'main source change')
            base = git('rev-parse', 'HEAD').stdout.strip()
            git('checkout', 'pr')
            self.assertEqual(git('merge', '--no-edit', 'main', check=False).returncode, 1)
            result = self.run_driver(root, base, 'restore')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(git('diff', '--name-only', '--diff-filter=U').stdout, 'source.rs\n')
            (root / 'source.rs').write_text('resolved source\n')
            git('add', 'source.rs')
            result = self.run_driver(root, base, 'generate')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(git('rev-parse', 'MERGE_HEAD').stdout.strip(), base)

    def test_failure_after_output_write_requires_generation_again(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, base = self.fixture(root, '    raise RuntimeError("after output write")\n')
            self.assertEqual(self.run_driver(root, base, 'restore').returncode, 0)
            result = self.run_driver(root, base, 'generate')
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'regenerated\n')
            self.assertEqual(git('rev-parse', 'MERGE_HEAD').stdout.strip(), base)
            # Retry runs the generator rather than treating a written product
            # as proof of success. Resolve the generator source for recovery.
            script = root / 'scripts/check-exec-context.py'
            script.write_text(script.read_text().replace('    raise RuntimeError("after output write")\n', ''))
            result = self.run_driver(root, base, 'generate')
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_template_mirror_uses_symlink_without_touching_unrelated_claude_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, _ = self.fixture(root)
            git('merge', '--abort')
            git('checkout', 'main')
            (root / 'aida-core/templates/commands').mkdir(parents=True)
            (root / 'aida-core/templates/commands/run.md').write_text('authoritative\n')
            (root / '.claude/commands').mkdir(parents=True)
            (root / '.claude/commands/run.md').symlink_to('../../aida-core/templates/commands/run.md')
            (root / '.claude/settings.json').write_text('local settings\n')
            (root / 'Makefile').write_text('sync-templates:\n\tln -sf ../../aida-core/templates/commands/run.md .claude/commands/run.md\n')
            git('add', '.')
            git('commit', '-qm', 'template mirror')
            base = git('rev-parse', 'HEAD').stdout.strip()
            git('checkout', 'pr')
            git('merge', '--no-edit', 'main', check=False)
            for phase in ('restore', 'generate'):
                result = self.run_driver(root, base, phase)
                self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((root / '.claude/commands/run.md').is_symlink())
            self.assertEqual((root / '.claude/commands/run.md').read_text(), 'authoritative\n')
            self.assertEqual((root / '.claude/settings.json').read_text(), 'local settings\n')

    def test_registry_excludes_local_extensions_and_unrelated_claude_files(self):
        self.assertTrue(driver.registered('.claude/skills/aida-test/SKILL.md', '120000'))
        self.assertFalse(driver.registered('.claude/commands/custom.md', '120000', {'aida-core/templates/commands/run.md'}))
        self.assertFalse(driver.registered('.aida/discipline/custom.md', '100644', {}))
        self.assertFalse(driver.registered('.claude/settings.json', '100644'))
        self.assertFalse(driver.registered('.claude/skills/local/SKILL.md', '120000'))
        self.assertFalse(driver.registered('.claude/commands/custom.local.md', '120000'))


if __name__ == '__main__':
    unittest.main()
