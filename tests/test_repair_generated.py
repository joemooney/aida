"""Real Git fixtures for the host-embedded regeneration driver.
trace:STORY-2710 | ai:codex
"""
import importlib.util
import os
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
            # The refused run's writes, registered or not, return to the index
            # so no later publication attempt can stage them.
            self.assertIn('rolled back', result.stderr)
            self.assertEqual((root / 'source.rs').read_text(), 'base\n')
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'main\n')

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
            # The partial product is rolled back to the staged (restored) bytes.
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'main\n')
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

    def test_generator_timeout_rolls_back_new_and_modified_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, base = self.fixture(root, '    Path("stray.txt").write_text("partial")\n'
                                           '    import time; time.sleep(30)\n')
            self.assertEqual(self.run_driver(root, base, 'restore').returncode, 0)
            result = subprocess.run(('python3', str(DRIVER), base, 'generate', '[]', '2'),
                                    cwd=root, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('timed out', result.stderr)
            self.assertFalse((root / 'stray.txt').exists())
            self.assertEqual((root / 'scripts/exec-context-legacy.json').read_text(), 'main\n')

    def test_pack_rule_is_closed_over_staged_masters(self):
        with tempfile.TemporaryDirectory() as tmp:
            old = os.getcwd()
            os.chdir(tmp)
            try:
                Path('aida-core/templates/skills/aida-x').mkdir(parents=True)
                Path('aida-core/templates/skills/aida-x/SKILL.md').write_text('m')
                Path('aida-core/templates/skills/aida-x/examples').mkdir()
                Path('aida-core/templates/skills/aida-x/examples/e.md').write_text('h')
                staged = {'aida-core/templates/skills/aida-x/SKILL.md',
                          'aida-core/templates/skills/aida-x/examples/e.md'}
                new = (0o644, 'h')
                inventory = driver.expected_pack_outputs(staged, ())
                self.assertEqual(inventory, {
                    ".agents/skills/aida-x/SKILL.md",
                    ".agents/skills/aida-x/examples/e.md"})
                allowed = driver.pack_change_allowed
                self.assertTrue(allowed('.agents/skills/aida-x/SKILL.md', None, new, inventory))
                self.assertTrue(allowed('.agents/skills/aida-x/examples/e.md', None, new, inventory))
                self.assertFalse(allowed('.agents/skills/aida-x/SKILL.md', None, new, set()))
                self.assertFalse(allowed('.agents/skills/aida-x/run.sh', None, new, inventory))
                self.assertFalse(allowed('.agents/skills/aida-x/SKILL.md', None, ('link', 'x'), inventory))
                self.assertFalse(allowed('.agents/skills/other/SKILL.md', None, new, inventory))
                self.assertTrue(allowed('.agents/skills/aida-old/SKILL.md', new, None, inventory))
                self.assertTrue(allowed('.agents/skills/aida-h/SKILL.md', None, new,
                                        {'.agents/skills/aida-h/SKILL.md'}))
            finally:
                os.chdir(old)

    def test_pack_inventory_requires_a_real_skill_prompt(self):
        with tempfile.TemporaryDirectory() as tmp:
            old = os.getcwd()
            os.chdir(tmp)
            try:
                sources = {
                    'aida-core/templates/skills-portable/aida-override.md',
                    'aida-core/templates/skills/aida-helper/examples/example.md',
                }
                for source in sources:
                    path = Path(source)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text('template')
                self.assertEqual(driver.expected_pack_outputs(sources, ()), set())
                prompt = 'aida-core/templates/skills/aida-helper/SKILL.md'
                Path(prompt).write_text('prompt')
                sources.add(prompt)
                self.assertEqual(driver.expected_pack_outputs(sources, ()), {
                    '.agents/skills/aida-helper/SKILL.md',
                    '.agents/skills/aida-helper/examples/example.md',
                })
            finally:
                os.chdir(old)

    def test_pack_inventory_is_frozen_before_generator_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            git, base = self.fixture(root)
            self.assertEqual(self.run_driver(root, base, 'restore').returncode, 0)
            master = root / 'aida-core/templates/skills/aida-late.md'
            master.parent.mkdir(parents=True)
            master.write_text('staged master')
            git('add', str(master.relative_to(root)))
            # A missing working-tree master cannot authorize an output. The
            # generator recreates the same staged bytes; the output inventory
            # must not expand in response to that recreation.
            master.unlink()
            (root / 'Makefile').write_text(
                'sync-templates:\n'
                '\tmkdir -p .agents/skills/aida-late\n'
                '\techo -n "staged master" > aida-core/templates/skills/aida-late.md\n'
                '\techo generated > .agents/skills/aida-late/SKILL.md\n')
            # Establish the absent master before generation; its recreation is
            # itself refused as well as its unauthorized portable output.
            git('add', 'Makefile')
            git('commit', '-qm', 'staged generator')
            base = git('rev-parse', 'HEAD').stdout.strip()
            result = self.run_driver(root, base, 'generate')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('.agents/skills/aida-late/SKILL.md', result.stderr)
            self.assertFalse((root / '.agents/skills/aida-late/SKILL.md').exists())

    def test_registry_excludes_local_extensions_and_unrelated_claude_files(self):
        self.assertTrue(driver.registered('.claude/skills/aida-test/SKILL.md', '120000'))
        self.assertFalse(driver.registered('.claude/commands/custom.md', '120000', {'aida-core/templates/commands/run.md'}))
        self.assertFalse(driver.registered('.aida/discipline/custom.md', '100644', {}))
        self.assertFalse(driver.registered('.claude/settings.json', '100644'))
        self.assertFalse(driver.registered('.claude/skills/local/SKILL.md', '120000'))
        self.assertFalse(driver.registered('.claude/commands/custom.local.md', '120000'))


if __name__ == '__main__':
    unittest.main()
