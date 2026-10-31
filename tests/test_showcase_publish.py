"""Behavioral checks for the shared GitHub/GitLab Pages site assembly."""
# trace:TASK-2604 | ai:claude
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / 'docs/showcase/publish.py'


class ShowcasePublishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'repo'
        site = self.root / 'docs/showcase'
        (site / 'assets').mkdir(parents=True)
        files = {
            'docs/showcase/index.html': '<a href="getting-started.html">a</a> <a href="../what-is-aida.html#top">b</a> '
                                        '<a href="../cli/02-specs.md#x">c</a> <a href="../../">d</a> '
                                        '<a href="https://example.com/">e</a> <a href="#local">f</a> <img src="assets/x.svg">',
            'docs/showcase/getting-started.html': '<a href="../../README.md#install">i</a>',
            'docs/showcase/page-template.html': 'template',
            'docs/showcase/styles.css': 'body{}',
            'docs/showcase/app.js': 'x',
            'docs/showcase/assets/x.svg': '<svg/>',
            'docs/showcase/build.py': '',
            'docs/showcase/publication.json': '{}',
            'docs/showcase/README.md': '',
            'docs/showcase/untracked.html': 'not staged',
            'docs/what-is-aida.html': '<a href="showcase/index.html">home</a> <a href="cli/02-specs.md">ref</a>',
        }
        for name, text in files.items():
            (self.root / name).write_text(text)
        self.git('init', '-q')
        self.git('add', *[n for n in files if 'untracked' not in n])
        self.git('-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qm', 'fixture')

    def git(self, *args):
        subprocess.run(['git', *args], cwd=self.root, check=True, capture_output=True)

    def run_publish(self, *args, expected=0):
        result = subprocess.run(['python3', str(SCRIPT), *args], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def test_selects_published_set_and_rewrites_links(self):
        out = Path(self.temp.name) / 'public'
        self.run_publish('--out', str(out))
        names = sorted(p.relative_to(out).as_posix() for p in out.rglob('*') if p.is_file())
        self.assertEqual(names, ['.nojekyll', 'app.js', 'assets/x.svg', 'getting-started.html',
                                 'index.html', 'overview.html', 'styles.css'])
        index = (out / 'index.html').read_text()
        self.assertIn('href="getting-started.html"', index)
        self.assertIn('href="overview.html#top"', index)
        self.assertIn('href="https://github.com/joemooney/aida/blob/main/docs/cli/02-specs.md#x"', index)
        self.assertIn('href="https://github.com/joemooney/aida"', index)
        self.assertIn('href="https://example.com/"', index)
        self.assertIn('href="#local"', index)
        self.assertIn('src="assets/x.svg"', index)
        self.assertIn('blob/main/README.md#install', (out / 'getting-started.html').read_text())
        overview = (out / 'overview.html').read_text()
        self.assertIn('href="index.html"', overview)
        self.assertIn('blob/main/docs/cli/02-specs.md', overview)

    def test_output_is_deterministic_and_compare_detects_drift(self):
        out = Path(self.temp.name) / 'public'
        self.run_publish('--out', str(out))
        self.run_publish('--compare', str(out))
        (out / 'styles.css').write_text('changed')
        (out / 'stray.html').write_text('extra')
        (out / 'app.js').unlink()
        result = self.run_publish('--compare', str(out), expected=1)
        self.assertIn('differs: styles.css', result.stdout)
        self.assertIn('extra:   stray.html', result.stdout)
        self.assertIn('missing: app.js', result.stdout)

    def test_refuses_non_empty_output_and_honours_source_override(self):
        out = Path(self.temp.name) / 'public'
        out.mkdir()
        (out / 'old.html').write_text('old')
        self.run_publish('--out', str(out), expected=1)
        other = Path(self.temp.name) / 'air-gapped'
        self.run_publish('--out', str(other), '--repo-url', 'https://git.local/aida',
                         '--blob-prefix', 'https://git.local/aida/-/blob/main/')
        index = (other / 'index.html').read_text()
        self.assertIn('href="https://git.local/aida/-/blob/main/docs/cli/02-specs.md#x"', index)
        self.assertIn('href="https://git.local/aida"', index)


if __name__ == '__main__':
    unittest.main()
