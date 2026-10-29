#!/usr/bin/env python3
"""Render the explicitly selected product guides; Python stdlib only.

This deliberately supports headings, paragraphs, quotes, lists, fenced code,
links, emphasis and inline code. Source Markdown remains authoritative.
"""
# trace:TASK-B6B1278890774EF993D2EBC327862470-106 | ai:codex
import html
import hashlib
import json
import subprocess
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / 'docs/showcase'
# trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex
MANIFEST = json.loads((SITE / 'publication.json').read_text())
PAGES = [(p['source'], p['output'], p['label']) for p in MANIFEST['pages']]



def inline(text):
    text = html.escape(text)
    text = re.sub(r'`([^`]+)`', r'<code>\1</code>', text)
    def link(match):
        label, target = match.groups()
        if target.startswith(('javascript:', 'data:', '//')):
            raise ValueError('Unsupported link scheme')
        if not target.startswith(('https:', 'http:', '#', 'mailto:')):
            path, marker, anchor = target.partition('#')
            mapped = {source: output for source, output, _ in PAGES}
            target = mapped.get(path, '../' + path) + (marker + anchor if marker else '')
        return f'<a href="{target}">{label}</a>'
    text = re.sub(r'\[([^\]]+)\]\(([^\s)]+)\)', link, text)
    text = re.sub(r'\*\*([^*]+)\*\*', r'<strong>\1</strong>', text)
    return text


def render(source):
    output, paragraph, listing, code, slugs = [], [], None, None, set()
    def flush():
        if paragraph:
            output.append('<p>' + inline(' '.join(paragraph)) + '</p>')
            paragraph.clear()
    def close_list():
        nonlocal listing
        if listing:
            output.append(f'</{listing}>')
            listing = None
    for line in source.splitlines():
        if line.startswith('```'):
            flush(); close_list()
            if code is None:
                code = []
            else:
                output.append('<pre><code>' + html.escape('\n'.join(code)) + '</code></pre>')
                code = None
            continue
        if code is not None:
            code.append(line); continue
        if line.startswith('<!--'):
            continue
        heading = re.match(r'^(#{1,6}) (.+)$', line)
        item = re.match(r'^[-*] (.+)$', line)
        if not line.strip() or heading or item or line.startswith('> '):
            flush()
        if item:
            if listing is None:
                listing = 'ul'; output.append('<ul>')
            output.append('<li>' + inline(item[1]) + '</li>'); continue
        close_list()
        if heading:
            level, title = len(heading[1]), heading[2]
            slug = re.sub(r'[^a-z0-9 -]', '', title.lower()).replace(' ', '-')
            if slug in slugs:
                raise ValueError('Duplicate heading: ' + title)
            slugs.add(slug)
            output.append(f'<h{level} id="{slug}">{inline(title)}</h{level}>')
        elif line.startswith('> '):
            output.append('<blockquote><p>' + inline(line[2:]) + '</p></blockquote>')
        elif line.strip():
            paragraph.append(line)
    flush(); close_list()
    if code is not None:
        raise ValueError('Unclosed code fence')
    return '\n'.join(output)


def main():
    template = (SITE / 'page-template.html').read_text()
    entries = []
    for (source, output, title), metadata in zip(PAGES, MANIFEST['pages']):
        if Path(output).name != output or not output.endswith('.html'):
            raise ValueError('Output must be an HTML basename')
        if metadata['category'] not in ('human', 'operator'):
            raise ValueError('Unknown audience category')
        source_path = ROOT / 'docs' / source
        source_path.resolve().relative_to((ROOT / 'docs').resolve())
        if source_path.suffix != '.md' or source_path.is_symlink():
            raise ValueError('Only ordinary Markdown sources may be selected')
        subprocess.run(['git', 'ls-files', '--error-unmatch', '--', str(source_path.relative_to(ROOT))], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
        content = source_path.read_text()
        text = re.sub(r'<!--.*?-->', '', content, flags=re.S)
        text = re.sub(r'[`#*>]', '', text)
        text = re.sub(r'\s+', ' ', text).strip()
        entries.append({'title': content.splitlines()[0].removeprefix('# '), 'url': output, 'source': '../' + source, 'category': metadata['category'], 'text': text, 'sha256': hashlib.sha256(content.encode()).hexdigest()})
        nav = '<a href="library.html">Library</a>' + ''.join(f'<a href="{dest}"' + (' aria-current="page"' if dest == output else '') + f'>{html.escape(label)}</a>' for _, dest, label in PAGES)
        graphic_path = SITE / 'assets' / (Path(output).stem + '.svg')
        graphic = f'<figure><img class="guide-graphic" src="assets/{graphic_path.name}" alt="{html.escape(title)} diagram; details follow in the source guide"></figure>' if graphic_path.exists() else ''
        interactive = ''
        scripts = ''
        # trace:TASK-B6B1278890774EF993D2EBC327862470-109 | ai:codex
        if output == 'autonomy.html':
            landing = (SITE / 'index.html').read_text()
            interactive = re.search(r'<section class="autonomy shell" id="autonomy">.*?</section>', landing, re.S).group(0)
            scripts = '<script src="app.js" defer></script>'
        page = template.replace('{{interactive}}', interactive).replace('{{scripts}}', scripts).replace('{{graphic}}', graphic).replace('{{title}}', html.escape(title)).replace('{{nav}}', nav).replace('{{source}}', '../' + source).replace('{{content}}', render(content))
        (SITE / output).write_text(page)
    write_library(template, entries)


def write_library(template, entries):
    # Only manifest-selected tracked source text enters this content index.
    cards = []
    for entry in entries:
        cards.append(f'<article class="library-card" data-url="{entry["url"]}" data-category="{entry["category"]}"><p class="eyebrow">{entry["category"]} guide</p><h2><a href="{entry["url"]}">{html.escape(entry["title"])}</a></h2><p class="excerpt">{html.escape(entry["text"][:180])}…</p><a href="{entry["source"]}">Source Markdown</a></article>')
    content = (SITE / 'library-template.html').read_text().replace('{{cards}}', '\n'.join(cards))
    nav = '<a href="library.html" aria-current="page">Library</a>' + ''.join(f'<a href="{output}">{html.escape(title)}</a>' for _, output, title in PAGES)
    scripts = '<script src="search-data.js" defer></script><script src="library.js" defer></script>'
    page = template.replace('{{title}}', 'Documentation library').replace('{{nav}}', nav).replace('{{source}}', 'publication.json').replace('{{content}}', content).replace('{{graphic}}', '').replace('{{interactive}}', '').replace('{{scripts}}', scripts)
    page = page.replace('the source guide', 'the publication manifest')
    (SITE / 'library.html').write_text(page)
    data = json.dumps(entries, ensure_ascii=True, sort_keys=True, indent=2).replace('<', '\\u003c')
    (SITE / 'search-data.js').write_text('// Generated by build.py; do not edit.\n// trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex\nwindow.AIDA_DOCS = ' + data + ';\n')


if __name__ == '__main__':
    main()
