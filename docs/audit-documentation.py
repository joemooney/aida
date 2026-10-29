#!/usr/bin/env python3
"""Snapshot tracked documentation and local-link candidates; not a CI gate.

This inventories files, not the truth of every sentence. Source/help review and
historical context remain human/agent judgments recorded in the companion report.
"""
# trace:TASK-B6B1278890774EF993D2EBC327862470-113 | ai:codex
import argparse
import csv
import hashlib
import json
import os
import re
import subprocess
from collections import Counter
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'docs/reports'
PREFIX = '2026-10-29-documentation'
EXCLUDED = {f'docs/reports/{PREFIX}{suffix}' for suffix in ('-inventory.csv', '-links.csv', '-snapshot.json', '-review.md')}
CURRENT = {'README.md', 'CLAUDE.md', 'AGENTS.md', 'OVERVIEW.md',
           'docs/getting-started.md', 'docs/first-project.md', 'docs/what-is-aida.md',
           'docs/what-is-aida.html', 'docs/configuration-guide.md', 'docs/product-workflow.md',
           'docs/selectable-autonomy.md', 'docs/trust-and-evidence.md', 'docs/metrics-guide.md',
           'docs/operator-setup.md', 'docs/admin-guide.md', 'docs/storage-modes.md',
           'docs/multi-node.md', 'docs/multi-hub-sync.md', 'docs/lifecycle.md'}


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True)


def category(name):
    if name.startswith(('docs/archive/', 'docs/plans/', 'docs/spikes/', 'docs/reports/', 'docs/testing/')):
        return 'historical-or-evidence'
    if '/05-decisions/' in name or Path(name).name.startswith('ADR'):
        return 'decision-record'
    if name.startswith(('docs/aida/', 'docs/presentation/')):
        return 'contextual-evidence'
    if name.startswith('aida-core/templates/'):
        return 'template-master'
    if name.startswith(('.claude/', '.agents/', '.codex/')):
        return 'derived-agent-mirror'
    if name.startswith('docs/showcase/') and name.endswith('.html') and not name.endswith('-template.html'):
        return 'current-static-page'
    if name.startswith('docs/showcase/') and not name.endswith('.md'):
        return 'site-source-or-asset'
    if name in CURRENT or name.startswith('docs/cli/') and name.endswith('.md'):
        return 'current-reader-reference'
    if name.endswith(('.md', '.rst', '.adoc')):
        return 'reference-requires-context'
    return 'documentation-tool-or-asset'


def without_code(text):
    text = re.sub(r'<!--.*?-->', lambda m: '\n' * m[0].count('\n'), text, flags=re.S)
    return re.sub(r'^\s*(`{3,}|~{3,}).*?^\s*\1\s*$', lambda m: '\n' * m[0].count('\n'), text, flags=re.M | re.S)


def anchors(path):
    text = path.read_text(errors='replace')
    result = set(re.findall(r'\b(?:id|name)=[\"\']([^\"\']+)', text))
    if path.suffix == '.md':
        used = Counter()
        for title in re.findall(r'^#{1,6}\s+(.+?)\s*#*$', without_code(text), re.M):
            title = re.sub(r'<[^>]+>', '', title).strip().lower()
            slug = re.sub(r'[^\w -]', '', title).replace(' ', '-')
            number = used[slug]; used[slug] += 1
            result.add(slug + (f'-{number}' if number else ''))
    return result


def links(name, text):
    if name.endswith('.md'):
        clean = without_code(text)
        clean = re.sub(r'(`+).*?\1', lambda m: ' ' * len(m[0]), clean)
        # Keep line numbers comparable to the original for ordinary prose links.
        for match in re.finditer(r'\[[^\]\n]*\]\(([^\s)]+)(?:\s+[^)]*)?\)', clean):
            yield clean.count('\n', 0, match.start()) + 1, match[1]
    elif name.endswith(('.html', '.svg')):
        for match in re.finditer(r'\b(?:href|src)=[\"\']([^\"\']+)', text):
            yield text.count('\n', 0, match.start()) + 1, match[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True, help='Reviewed stack base commit, for reproducible context')
    args = parser.parse_args()
    base = git('rev-parse', args.base + '^{commit}').strip()
    tracked = git('ls-files', '-z').split('\0')
    names = sorted(n for n in tracked if n and n not in EXCLUDED and
                   (n.startswith('docs/') or n.endswith(('.md', '.rst', '.adoc'))))
    dates = {}; current_date = ''
    for line in git('log', '--format=@@%cI', '--name-only', base, '--', '*.md', '*.rst', '*.adoc', 'docs/').splitlines():
        if line.startswith('@@'):
            current_date = line[2:]
        elif line in names and line not in dates:
            dates[line] = current_date
    inventory = []; failures = []; counts = Counter(); checked_links = 0
    anchor_cache = {}
    for name in names:
        path = ROOT / name
        if not path.exists() and not path.is_symlink():
            continue
        kind = category(name); counts[kind] += 1
        data = os.readlink(path).encode() if path.is_symlink() else path.read_bytes()
        text = data.decode(errors='replace') if path.suffix in ('.md', '.html', '.svg', '.rst', '.adoc') else ''
        hype = sorted(set(re.findall(r'\b(?:revolutionary|game-changing|cutting-edge|best-in-class|world-class|enterprise-grade|blazing fast|seamless)\b', text, re.I)))
        inventory.append({'file': name, 'classification': kind, 'sha256': hashlib.sha256(data).hexdigest(),
                          'last_git_change': dates.get(name, 'in-this-PR-or-not-in-reviewed-base'),
                          'semantic_review': 'focused-source/help-review-see-report' if name in CURRENT or name.startswith('docs/showcase/') else 'not-certified-by-inventory',
                          'tone_candidates': '; '.join(hype)})
        if path.is_symlink():
            continue
        for line, target in links(name, text):
            target = target.replace('&amp;', '&')
            url = urlsplit(target)
            if url.scheme or url.netloc or any(c in target for c in '<>{}'):
                continue
            checked_links += 1
            dest = (path.parent / unquote(url.path)).resolve() if url.path else path.resolve()
            reason = ''
            try:
                dest.relative_to(ROOT)
            except ValueError:
                failures.append({'file': name, 'line_candidate': line, 'classification': kind, 'target': target, 'reason': 'outside-repository'})
                continue
            if not dest.exists():
                reason = 'missing-path'
            elif url.fragment and dest.is_file() and dest.suffix in ('.md', '.html', '.svg'):
                if dest not in anchor_cache:
                    anchor_cache[dest] = anchors(dest)
                if unquote(url.fragment) not in anchor_cache[dest]:
                    reason = 'anchor-candidate'
            if reason:
                failures.append({'file': name, 'line_candidate': line, 'classification': kind, 'target': target, 'reason': reason})
    OUT.mkdir(exist_ok=True)
    for suffix, records, fields in [('-inventory.csv', inventory, ['file','classification','sha256','last_git_change','semantic_review','tone_candidates']),
                                    ('-links.csv', failures, ['file','line_candidate','classification','target','reason'])]:
        with (OUT / (PREFIX + suffix)).open('w', newline='') as output:
            writer = csv.DictWriter(output, fieldnames=fields); writer.writeheader(); writer.writerows(records)
    snapshot = {'reviewed_base': base, 'date': '2026-10-29', 'tracked_documentation_files': len(inventory),
                'classification_counts': dict(sorted(counts.items())), 'local_links_scanned': checked_links,
                'link_candidates': len(failures), 'self_derived_outputs_excluded_from_hash_cycle': sorted(EXCLUDED), 'scope': 'Post-fix working-tree hashes; Git history metadata from reviewed_base. Candidate scan is not semantic certification or external URL validation. Historical records remain unchanged.'}
    (OUT / (PREFIX + '-snapshot.json')).write_text(json.dumps(snapshot, indent=2) + '\n')
    print(json.dumps(snapshot, indent=2))
    focused = [f for f in failures if f['file'] in CURRENT or f['file'].startswith('docs/showcase/')]
    print('FOCUSED LINK CANDIDATES:', json.dumps(focused, indent=2))


if __name__ == '__main__':
    main()
