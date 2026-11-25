#!/usr/bin/env python3
"""Render the explicitly selected product guides; Python stdlib only.

This deliberately supports headings, paragraphs, quotes, lists, fenced code,
links, bold and inline code. Source Markdown remains authoritative.
"""
# trace:TASK-B6B1278890774EF993D2EBC327862470-106 | ai:codex
import argparse
import html
import hashlib
import json
import posixpath
import subprocess
import re
import xml.etree.ElementTree as ET
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / 'docs/showcase'
# trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex
MANIFEST = json.loads((SITE / 'publication.json').read_text())
PAGES = [(p['source'], p['output'], p['label']) for p in MANIFEST['pages']]
# trace:STORY-2219 | ai:claude
# The product glossary's single source is the discipline-pack template master,
# which the CLI glossary embeds too, so it is the one source allowed outside
# docs/. Every other selected source must live under docs/.
GLOSSARY_SOURCE_DIR = ROOT / 'aida-core/templates/.aida/discipline'

# trace:STORY-2839 | ai:antigravity
NAV_GROUP_ORDER = ('How it works', 'Governance', 'Operations', 'Reference')
NAV_GROUPS = {
    'workflow.html': 'How it works',
    'autonomy.html': 'How it works',
    'capabilities.html': 'How it works',
    'code-review.html': 'How it works',
    'authority.html': 'Governance',
    'trust.html': 'Governance',
    'metrics.html': 'Governance',
    'engineering-audit.html': 'Governance',
    'setup.html': 'Operations',
    'configuration.html': 'Operations',
    'integrations.html': 'Operations',
    'air-gapped.html': 'Operations',
    'library.html': 'Reference',
    'glossary.html': 'Reference',
    'features.html': 'Reference',
    'executive-brief.html': 'Reference',
}
EXTRA_NAV_PAGES = (
    ('code-review.html', 'Code review'),
    ('engineering-audit.html', 'Engineering audit'),
    ('features.html', 'All features'),
    ('executive-brief.html', 'Executive brief'),
)


def build_nav(current):
    """Render top-level direct links and grouped dropdown menus for guide headers."""
    all_items = [('library.html', 'Library')] + [(dest, label) for _, dest, label in PAGES]
    for dest, label in EXTRA_NAV_PAGES:
        if (SITE / dest).is_file():
            all_items.append((dest, label))
    direct = []
    grouped = {group: [] for group in NAV_GROUP_ORDER}
    for dest, label in all_items:
        group = NAV_GROUPS.get(dest)
        if group in grouped:
            grouped[group].append((dest, label))
        else:
            direct.append((dest, label))
    parts = [
        f'<a href="{dest}"' + (' aria-current="page"' if dest == current else '') + f'>{html.escape(label)}</a>'
        for dest, label in direct
    ]
    for group in NAV_GROUP_ORDER:
        items = grouped[group]
        if not items:
            continue
        is_active = any(dest == current for dest, _ in items)
        active_cls = ' nav-group-active' if is_active else ''
        links = ''.join(
            f'<a href="{dest}"' + (' aria-current="page"' if dest == current else '') + f'>{html.escape(label)}</a>'
            for dest, label in items
        )
        parts.append(
            f'<div class="nav-group{active_cls}">'
            f'<button type="button" class="nav-trigger" aria-expanded="false">{html.escape(group)} <span class="nav-caret" aria-hidden="true">▾</span></button>'
            f'<div class="nav-menu">{links}</div>'
            f'</div>'
        )
    return ''.join(parts)


def slug(title):
    return re.sub(r'[^a-z0-9 -]', '', title.lower()).replace(' ', '-')


def inline(text, base='.'):
    """Render inline Markdown; `base` is the source's directory relative to docs/."""
    text = html.escape(text)
    text = re.sub(r'`([^`]+)`', r'<code>\1</code>', text)
    def link(match):
        label, target = match.groups()
        if target.startswith(('javascript:', 'data:', '//')):
            raise ValueError('Unsupported link scheme')
        if not target.startswith(('https:', 'http:', '#', 'mailto:')):
            path, marker, anchor = target.partition('#')
            docs = str(ROOT / 'docs')
            path = posixpath.relpath(posixpath.normpath(posixpath.join(docs, base, path)), docs)
            mapped = {source: output for source, output, _ in PAGES}
            mapped["what-is-aida.md"] = "../what-is-aida.html"
            mapped["showcase/code-review.html"] = "code-review.html"
            target = mapped.get(path, '../' + path) + (marker + anchor if marker else '')
        return f'<a href="{target}">{label}</a>'
    text = re.sub(r'\[([^\]]+)\]\(([^\s)]+)\)', link, text)
    text = re.sub(r'\*\*([^*]+)\*\*', r'<strong>\1</strong>', text)
    return text


def graphic_description(graphic_path, fallback):
    """Alt text for an SVG asset: its <title> and <desc>, else the fallback."""
    svg = ET.fromstring(graphic_path.read_text())
    # trace:TASK-HOME-128 | ai:codex
    description = '. '.join(' '.join(node.itertext()).strip().rstrip('.')
                           for tag in ('title', 'desc')
                           for node in svg.findall('{http://www.w3.org/2000/svg}' + tag))
    return description + '.' if description else fallback


def figure(graphic_path, fallback):
    description = graphic_description(graphic_path, fallback)
    return f'<figure><img class="guide-graphic" src="assets/{graphic_path.name}" alt="{html.escape(description, quote=True)}"></figure>'


# trace:TASK-B6B1278890774EF993D2EBC327862470-277 | ai:claude
# A line holding only `![label](showcase/assets/<name>.svg)` places that
# diagram in the page body. The path is relative to docs/, so the Markdown
# source also renders the image when read on the forge.
FIGURE = re.compile(r'^!\[([^\]]+)\]\(([^)]+\.svg)\)$')


def parse_table_row(line):
    return [cell.strip() for cell in line.strip().strip('|').split('|')]


def is_table_separator(line):
    cells = parse_table_row(line)
    return bool(cells) and all(re.fullmatch(r':?-{3,}:?', c) for c in cells)


def render(source, base='.'):
    output, paragraph, listing, table, code, slugs = [], [], None, None, None, set()
    def flush():
        if paragraph:
            output.append('<p>' + inline(' '.join(paragraph), base) + '</p>')
            paragraph.clear()
    def close_list():
        nonlocal listing
        if listing:
            output.append(f'</{listing}>')
            listing = None
    def close_table():
        nonlocal table
        if table:
            headers, rows = table
            thead = '<thead><tr>' + ''.join(f'<th>{inline(h, base)}</th>' for h in headers) + '</tr></thead>'
            tbody_rows = []
            for row in rows:
                padded = (row + [''] * len(headers))[:len(headers)]
                tbody_rows.append('<tr>' + ''.join(f'<td>{inline(c, base)}</td>' for c in padded) + '</tr>')
            tbody = '<tbody>' + ''.join(tbody_rows) + '</tbody>'
            output.append('<div class="table-wrap"><table>' + thead + tbody + '</table></div>')
            table = None
    lines = source.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith('```'):
            flush(); close_list(); close_table()
            if code is None:
                code = []
            else:
                output.append('<pre><code>' + html.escape('\n'.join(code)) + '</code></pre>')
                code = None
            i += 1
            continue
        if code is not None:
            code.append(line); i += 1; continue
        if line.startswith('<!--'):
            i += 1; continue
        if line.strip().startswith('|') and line.strip().endswith('|'):
            if table is None and i + 1 < len(lines) and lines[i + 1].strip().startswith('|') and is_table_separator(lines[i + 1]):
                flush(); close_list()
                table = (parse_table_row(line), [])
                i += 2
                continue
            elif table is not None:
                table[1].append(parse_table_row(line))
                i += 1
                continue
        close_table()
        image = FIGURE.match(line)
        if image:
            flush(); close_list()
            # trace:STORY-1651 | ai:codex — moved guides resolve figures from their source directory.
            graphic_path = (ROOT / 'docs' / base / image[2]).resolve()
            if graphic_path.parent != (SITE / 'assets').resolve() or not re.fullmatch(r'[a-z0-9-]+\.svg', graphic_path.name):
                raise ValueError('Figures must name showcase/assets/<name>.svg: ' + image[2])
            if not graphic_path.is_file():
                raise ValueError('Missing figure asset: ' + image[2])
            output.append(figure(graphic_path, image[1]))
            i += 1
            continue
        if line.startswith('!['):
            raise ValueError('Figures must be a whole line naming showcase/assets/<name>.svg: ' + line)
        heading = re.match(r'^(#{1,6}) (.+)$', line)
        item = re.match(r'^[-*] (.+)$', line)
        if not line.strip() or heading or item or line.startswith('> '):
            flush()
        if item:
            if listing is None:
                listing = 'ul'; output.append('<ul>')
            output.append('<li>' + inline(item[1], base) + '</li>')
            i += 1
            continue
        close_list()
        if heading:
            level, title = len(heading[1]), heading[2]
            anchor = slug(title)
            if anchor in slugs:
                raise ValueError('Duplicate heading: ' + title)
            slugs.add(anchor)
            output.append(f'<h{level} id="{anchor}">{inline(title, base)}</h{level}>')
        elif line.startswith('> '):
            output.append('<blockquote><p>' + inline(line[2:], base) + '</p></blockquote>')
        elif line.strip():
            paragraph.append(line)
        i += 1
    flush(); close_list(); close_table()
    if code is not None:
        raise ValueError('Unclosed code fence')
    return '\n'.join(output)


def search_text(content):
    text = re.sub(r'<!--.*?-->', '', content, flags=re.S)
    text = re.sub(r'^!\[([^\]]+)\]\([^\s)]+\)$', r'\1', text, flags=re.M)
    text = re.sub(r'\[([^\]]+)\]\([^\s)]+\)', r'\1', text)
    text = re.sub(r'^#{1,6} ', '', text, flags=re.M)
    text = re.sub(r'^> ?', '', text, flags=re.M)
    text = re.sub(r'^\|(?:\s*:?-{3,}:?\s*\|)+\s*$', ' ', text, flags=re.M)
    text = text.replace('|', ' ')
    text = text.replace('`', '').replace('**', '')
    return re.sub(r'\s+', ' ', text).strip()


# trace:STORY-2219 | ai:claude
def glossary_terms(content, output, source, category):
    """One search record per `###` entry under the glossary's `## Terms` heading."""
    terms, title, body, in_terms = [], None, [], False
    def close():
        if title is not None:
            section = '### ' + title + '\n' + '\n'.join(body)
            terms.append({'category': category, 'kind': 'term', 'title': title, 'url': output + '#' + slug(title), 'source': '../' + source, 'text': search_text('\n'.join(body)), 'sha256': hashlib.sha256(section.encode()).hexdigest()})
    for line in content.splitlines():
        if line.startswith('## '):
            close(); title, body = None, []
            in_terms = line == '## Terms'
        elif in_terms and line.startswith('### '):
            close(); title, body = line[4:], []
        elif title is not None:
            body.append(line)
    close()
    if not terms:
        raise ValueError('Glossary has no ### entries under ## Terms: ' + source)
    return terms


def render_outputs():
    """Collect all derived content before checking or writing any output."""
    outputs = {}
    seen = {"library.html", "index.html"}
    template = (SITE / 'page-template.html').read_text()
    entries, terms = [], []
    for (source, output, title), metadata in zip(PAGES, MANIFEST['pages']):
        if Path(output).name != output or not output.endswith('.html'):
            raise ValueError('Output must be an HTML basename')
        if output in seen:
            raise ValueError('Duplicate or reserved output: ' + output)
        seen.add(output)
        if metadata['category'] not in ('human', 'operator'):
            raise ValueError('Unknown audience category')
        source_path = ROOT / 'docs' / source
        resolved = source_path.resolve()
        is_glossary = bool(metadata.get('glossary'))
        allowed = GLOSSARY_SOURCE_DIR if is_glossary else ROOT / 'docs'
        if is_glossary and resolved.parent != allowed.resolve():
            raise ValueError('A glossary source must be in ' + str(allowed.relative_to(ROOT)))
        resolved.relative_to(allowed.resolve())
        if source_path.suffix != '.md' or source_path.is_symlink():
            raise ValueError('Only ordinary Markdown sources may be selected')
        subprocess.run(['git', 'ls-files', '--error-unmatch', '--', str(resolved.relative_to(ROOT.resolve()))], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
        content = source_path.read_text()
        entries.append({'title': content.splitlines()[0].removeprefix('# '), 'url': output, 'source': '../' + source, 'category': metadata['category'], 'text': search_text(content), 'sha256': hashlib.sha256(content.encode()).hexdigest()})
        if is_glossary:
            terms.extend(glossary_terms(content, output, source, metadata['category']))
        nav = build_nav(output)
        graphic_path = SITE / 'assets' / (Path(output).stem + '.svg')
        graphic = ''
        if graphic_path.exists():
            graphic = figure(graphic_path, title)
        interactive = ''
        scripts = '<script src="search-data.js" defer></script>'
        # trace:TASK-B6B1278890774EF993D2EBC327862470-109 | ai:codex
        if output == 'autonomy.html':
            landing = (SITE / 'index.html').read_text()
            interactive = re.search(r'<section class="autonomy shell" id="autonomy">.*?</section>', landing, re.S).group(0)
            scripts += '<script src="app.js" defer></script>'
        # trace:TASK-B6B1278890774EF993D2EBC327862470-113 | ai:codex
        heading, separator, body = render(content, posixpath.dirname(source) or '.').partition('</h1>')
        if not separator or not heading.startswith('<h1 '):
            raise ValueError('Guide must start with a level-one heading: ' + source)
        page = template.replace('{{interactive}}', interactive).replace('{{scripts}}', scripts).replace('{{heading}}', heading + separator).replace('{{graphic}}', graphic).replace('{{provenance}}', 'the source guide').replace('{{title}}', html.escape(title)).replace('{{nav}}', nav).replace('{{source}}', '../' + source).replace('{{content}}', body)
        outputs[SITE / output] = page
    outputs.update(render_library(template, entries, terms))
    return outputs


def render_library(template, entries, terms=()):
    # Only manifest-selected tracked source text enters this content index.
    cards = []
    for entry in entries:
        cards.append(f'<article class="library-card" data-url="{entry["url"]}" data-category="{entry["category"]}"><p class="eyebrow">{entry["category"]} guide</p><h2><a href="{entry["url"]}">{html.escape(entry["title"])}</a></h2><p class="excerpt">{html.escape(entry["text"][:180])}…</p><a href="{entry["source"]}">Source Markdown</a></article>')
    # trace:STORY-2219 | ai:claude
    # Glossary terms are searchable but stay hidden until a search matches them.
    for term in terms:
        cards.append(f'<article class="library-card term-card" data-url="{term["url"]}" data-category="{term["category"]}" data-term hidden><p class="eyebrow">glossary term</p><h2><a href="{term["url"]}">{html.escape(term["title"])}</a></h2><p class="excerpt">{html.escape(term["text"][:180])}…</p></article>')
    entries = list(entries) + list(terms)
    content = (SITE / 'library-template.html').read_text().replace('{{cards}}', '\n'.join(cards))
    nav = build_nav('library.html')
    scripts = '<script src="search-data.js" defer></script><script src="library.js" defer></script>'
    page = template.replace('{{heading}}', '').replace('{{title}}', 'Documentation library').replace('{{nav}}', nav).replace('{{source}}', 'publication.json').replace('{{content}}', content).replace('{{graphic}}', '').replace('{{interactive}}', '').replace('{{scripts}}', scripts)
    page = page.replace('{{provenance}}', 'the publication manifest')
    data = json.dumps(entries, ensure_ascii=True, sort_keys=True, indent=2).replace('<', '\\u003c')
    return {SITE / 'library.html': page, SITE / 'search-data.js': ('// Generated by build.py; do not edit.\n// trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex\nwindow.AIDA_DOCS = ' + data + ';\n')}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Report generated-file drift without writing files')
    args = parser.parse_args(argv)
    outputs = render_outputs()
    if args.check:
        # Check tracked publication artifacts; no generated writes occur in this mode.
        paths = [str(path.relative_to(ROOT)) for path in outputs]
        subprocess.run(['git', 'ls-files', '--error-unmatch', '--', *paths], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
        drifted = [path for path, content in outputs.items()
                   if not path.is_file() or path.read_bytes() != content.encode('utf-8')]
        for path in drifted:
            print('Generated output drift: ' + str(path.relative_to(ROOT)), file=sys.stderr)
        return int(bool(drifted))
    for path, content in outputs.items():
        path.write_text(content, encoding='utf-8')
    return 0


if __name__ == '__main__':
    sys.exit(main())
