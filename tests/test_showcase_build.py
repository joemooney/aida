"""Behavioral checks for read-only generated publication verification."""
# trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


class ShowcaseBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.site = self.root / 'docs/showcase'
        self.site.mkdir(parents=True)
        for name in ('build.py', 'page-template.html', 'library-template.html'):
            shutil.copyfile(REPO / 'docs/showcase' / name, self.site / name)
        self.source = self.root / 'docs/guide.md'
        self.source.write_text('# Fixture guide\n\nRead [a linked label](elsewhere.md). C# and A > B. The phrase the source guide belongs here.\n')
        self.manifest = {'version': 1, 'pages': [{'source': 'guide.md', 'output': 'guide.html', 'label': 'Guide', 'category': 'human'}]}
        self.save_manifest()
        self.git('init', '-q')
        self.git('add', '.')
        self.build()
        self.git('add', '.')

    def git(self, *args):
        subprocess.run(['git', *args], cwd=self.root, check=True, capture_output=True)

    def save_manifest(self):
        (self.site / 'publication.json').write_text(json.dumps(self.manifest))

    def build(self, *args, expected=0):
        result = subprocess.run(['python3', str(self.site / 'build.py'), *args], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def snapshot(self):
        return {p.relative_to(self.site): (p.read_bytes(), p.stat().st_mtime_ns) for p in self.site.rglob('*') if p.is_file()}

    def test_clean_check_and_rebuild_are_stable(self):
        before = self.snapshot()
        self.build('--check')
        self.assertEqual(before, self.snapshot())
        self.build()
        self.assertEqual({p: b for p, (b, _) in before.items()}, {p: b for p, (b, _) in self.snapshot().items()})

    def test_check_names_all_drift_and_never_writes(self):
        self.source.write_text(self.source.read_text() + '\nChanged source.\n')
        before = self.snapshot()
        result = self.build('--check', expected=1)
        for name in ('guide.html', 'library.html', 'search-data.js'):
            self.assertIn('docs/showcase/' + name, result.stderr)
        self.assertEqual(before, self.snapshot())

    def test_missing_tracked_output_is_reported_without_recreation(self):
        (self.site / 'guide.html').unlink()
        before = self.snapshot()
        self.assertIn('guide.html', self.build('--check', expected=1).stderr)
        self.assertEqual(before, self.snapshot())

    def test_reserved_and_duplicate_outputs_refuse_before_writes(self):
        for output in ('index.html', 'library.html', 'guide.html'):
            with self.subTest(output=output):
                self.manifest['pages'] = [self.manifest['pages'][0], {**self.manifest['pages'][0], 'output': output}]
                self.save_manifest()
                before = self.snapshot()
                self.assertIn('Duplicate or reserved output', self.build(expected=1).stderr)
                self.assertEqual(before, self.snapshot())

    def test_search_keeps_punctuation_and_link_labels(self):
        script = (self.site / 'search-data.js').read_text()
        records = json.loads(script.split('window.AIDA_DOCS = ', 1)[1].removesuffix(';\n'))
        text = records[0]['text']
        self.assertIn('a linked label', text)
        self.assertNotIn('elsewhere.md', text)
        self.assertIn('C# and A > B', text)
        self.assertNotIn('[', text)

    def test_footer_provenance_only_changes_placeholder(self):
        # A phrase in library content must survive substitution unchanged.
        template = self.site / 'library-template.html'
        template.write_text(template.read_text() + '<p>the source guide</p>')
        self.build()
        library = (self.site / 'library.html').read_text()
        self.assertIn('<p>the source guide</p>', library)
        self.assertIn('the publication manifest</a>', library)
        guide = (self.site / 'guide.html').read_text()
        self.assertIn('the source guide belongs here.', guide)
        self.assertIn('the source guide</a>', guide)

    # trace:TASK-HOME-128 | ai:codex
    def test_graphic_description_follows_source_heading(self):
        assets = self.site / 'assets'
        assets.mkdir()
        (assets / 'guide.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg"><title>Scope &amp; choices</title><desc>A &lt; B; distinct authority.</desc></svg>')
        self.build()
        page = (self.site / 'guide.html').read_text()
        self.assertIn('alt="Scope &amp; choices. A &lt; B; distinct authority."', page)
        self.assertLess(page.index('</h1>'), page.index('<figure>'))
        self.build('--check')

    # trace:TASK-B6B1278890774EF993D2EBC327862470-277 | ai:claude
    def test_body_figure_uses_svg_text_and_stays_out_of_search_markup(self):
        assets = self.site / 'assets'
        assets.mkdir()
        (assets / 'race.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg"><title>Two pushes</title><desc>One wins.</desc></svg>')
        self.source.write_text('# Fixture guide\n\nBefore.\n\n![Race diagram](showcase/assets/race.svg)\n\nAfter.\n')
        self.build()
        page = (self.site / 'guide.html').read_text()
        self.assertIn('<figure><img class="guide-graphic" src="assets/race.svg" alt="Two pushes. One wins."></figure>', page)
        self.assertLess(page.index('Before.'), page.index('<figure>'))
        self.assertLess(page.index('<figure>'), page.index('After.'))
        script = (self.site / 'search-data.js').read_text()
        self.assertIn('Before. Race diagram After.', script)
        self.assertNotIn('race.svg', script)

    def test_body_figure_refuses_missing_or_foreign_assets(self):
        for line in ('![Gone](showcase/assets/gone.svg)', '![Remote](https://example.com/x.svg)'):
            with self.subTest(line=line):
                self.source.write_text('# Fixture guide\n\n' + line + '\n')
                before = self.snapshot()
                self.build(expected=1)
                self.assertEqual(before, self.snapshot())

    @unittest.skipUnless(shutil.which('node'), 'Node is needed for the DOM fixture')
    def test_unicode_excerpt_and_literal_query(self):
        script = r'''
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const excerpt = {textContent: ''};
const entry = {url: 'guide.html', category: 'human', text: 'İ'.repeat(70) + ' needle literal [x] tail'};
const card = {dataset: {url: entry.url}, querySelector: () => excerpt};
const nodes = Object.fromEntries(['library-search', 'query', 'category', 'search-status', 'search-empty'].map(id => [id, {value: '', addEventListener() {}}]));
nodes.category.value = 'all'; nodes.query.value = 'needle';
const context = {window: {AIDA_DOCS: [entry]}, document: {getElementById: id => nodes[id], querySelectorAll: () => [card]}, setTimeout};
vm.createContext(context);
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
assert.equal(excerpt.textContent, '…' + entry.text.slice(entry.text.indexOf('needle') - 45) + '…');
nodes.query.value = '[x]'; vm.runInContext('filterGuides()', context);
assert.equal(card.hidden, false);
assert.ok(excerpt.textContent.includes('[x]'));
'''
        subprocess.run(['node', '-e', script, str(REPO / 'docs/showcase/library.js')], check=True, capture_output=True)


if __name__ == '__main__':
    unittest.main()
