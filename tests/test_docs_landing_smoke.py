"""Run the adopter landing page in an isolated project with an installed CLI.

Run: AIDA_TEST_BIN=/absolute/path/to/aida python3 -m unittest tests/test_docs_landing_smoke.py
The CLI is the installation prerequisite; no contributor document is opened.
"""
# trace:STORY-1651 | ai:codex
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LandingSmoke(unittest.TestCase):
    def test_landing_commands_create_and_show_first_spec(self):
        binary = os.environ.get('AIDA_TEST_BIN') or shutil.which('aida')
        self.assertTrue(binary, 'Install AIDA as linked by the landing page first')
        landing = (ROOT / 'docs/README.md').read_text()
        commands = re.search(r'```sh\n(.*?)\n```', landing, re.S).group(1)
        self.assertNotIn('internal/', commands)
        with tempfile.TemporaryDirectory(prefix='aida-landing-') as tmp:
            base = Path(tmp)
            (base / 'bin').mkdir()
            (base / 'bin/aida').symlink_to(Path(binary).resolve())
            (base / 'home').mkdir()
            # Remove launch/store/role context: this must be a new adopter project.
            env = {k: v for k, v in os.environ.items()
                   if not k.startswith(('AIDA_', 'GIT_'))}
            env.update(HOME=str(base / 'home'), XDG_CONFIG_HOME=str(base / 'config'),
                       XDG_CACHE_HOME=str(base / 'cache'), XDG_DATA_HOME=str(base / 'data'),
                       PATH=str(base / 'bin') + os.pathsep + os.environ['PATH'],
                       GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                       GIT_AUTHOR_NAME='Landing reader', GIT_AUTHOR_EMAIL='reader@example.test',
                       GIT_COMMITTER_NAME='Landing reader', GIT_COMMITTER_EMAIL='reader@example.test')
            run = subprocess.run(['sh', '-ec', commands], cwd=base, env=env,
                                 capture_output=True, text=True, timeout=45)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn('Make my first change', run.stdout)
            matches = re.findall(r'\b(?:TASK|WORK)-[A-Za-z0-9-]+', run.stdout)
            match = matches[-1] if matches else None
            self.assertIsNotNone(match, run.stdout)
            show = subprocess.run([binary, 'show', match], cwd=base / 'my-project',
                                  env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(show.returncode, 0, show.stdout + show.stderr)
            self.assertIn('Make my first change', show.stdout)
            print('Landing smoke: git init → aida init → add → list → show: PASS')


if __name__ == '__main__':
    unittest.main()
