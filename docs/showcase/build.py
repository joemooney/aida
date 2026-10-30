#!/usr/bin/env python3
"""Render the explicitly selected product guides; Python stdlib only.

This deliberately supports headings, paragraphs, quotes, lists, fenced code,
links, emphasis and inline code. Source Markdown remains authoritative.
"""
# trace:TASK-B6B1278890774EF993D2EBC327862470-106 | ai:codex
import html
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / 'docs/showcase'
PAGES = [('getting-started.md', 'getting-started.html', 'Get started'),
         ('configuration-guide.md', 'configuration.html', 'Configuration'),
         ('product-workflow.md', 'workflow.html', 'Workflow'),
         ('selectable-autonomy.md', 'autonomy.html', 'Autonomy'),
         ('trust-and-evidence.md', 'trust.html', 'Trust')]


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
            mapped["what-is-aida.md"] = "../what-is-aida.html"
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
    for source, output, title in PAGES:
        content = (ROOT / 'docs' / source).read_text()
        nav = ''.join(f'<a href="{dest}"' + (' aria-current="page"' if dest == output else '') + f'>{html.escape(label)}</a>' for _, dest, label in PAGES)
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


if __name__ == '__main__':
    main()
