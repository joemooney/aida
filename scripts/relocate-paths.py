#!/usr/bin/env python3
"""Re-key inventories after splitting a Rust source file, preserving site budgets.

Usage: python3 scripts/relocate-paths.py OLD NEW [NEW ...]
Run before regenerating store-writers.toml. Destinations must already exist;
OLD must be absent. Uses the existing inventory lexers, never line numbers.
Review source-inspection tests separately: their semantic assertions cannot
be relocated by a generic path replacement. Config-trust reader metadata is
retained per destination with the original channels and conserved site counts.
Other inventoried ledger formats refuse when OLD appears; relocate those
manually before rerunning this helper.
"""
# trace:TASK-3426 | ai:codex
import argparse
from collections import Counter
import importlib.util
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def relocate(root, old, destinations):
    for name in [old, *destinations]:
        path = Path(name)
        if path.is_absolute() or '..' in path.parts or not (root / path).resolve().is_relative_to(root.resolve()):
            raise ValueError('paths must stay inside the repository')
    if (root / old).exists():
        raise ValueError('source must have been moved before re-keying')
    # These inventories have distinct schemas and need a reviewed mapping.
    # Refuse before any writes rather than silently leaving stale path keys.
    for ledger in (
        'scripts/portability-allowlist.txt',
        'scripts/doc-path-exceptions.json',
        'scripts/git-maintenance-baseline.txt',
        'scripts/ci-gate-tiers.toml',
        'aida-cli-lib/src/tests/fixtures/proc_identity_legacy_baseline.json',
    ):
        path = root / ledger
        if path.exists() and old in path.read_text():
            raise ValueError(f'manual relocation required: {old} appears in {ledger}')
    writer = load('check-store-writers')
    env = load('check-exec-context')
    globs = load('check-crate-globs')
    sources = {name: (root / name).read_text() for name in destinations}
    functions = {name: {item: source[start:end] for start, end, item in writer.functions(writer.mask_rust(source))}
                 for name, source in sources.items()}
    for name, source in sources.items():
        masked = writer.mask_rust(source)
        for match in re.finditer(r'\b(?:struct|enum)\s+(\w+)[^{;]*\{', masked):
            functions[name][match.group(1)] = source[match.start():writer.block_end(masked, match.end() - 1)]
    env_sites = {name: Counter(env.rust_sites(source)) for name, source in sources.items()}
    updates = {}
    for ledger in ('exec-context-legacy.json', 'ownership-liveness-legacy.json', 'crate-globs-legacy.json'):
        path = root / 'scripts' / ledger
        data = json.loads(path.read_text())
        sites = data.get('sites', data)
        result = {key: count for key, count in sites.items() if key != old and not key.startswith(old + ':')}
        for key, count in sites.items():
            if key != old and not key.startswith(old + ':'):
                continue
            suffix = key[len(old):].lstrip(':')
            if ledger == 'crate-globs-legacy.json':
                found = {name: globs.count(source) for name, source in sources.items()}
            elif ledger == 'exec-context-legacy.json':
                found = {name + ':' + suffix: entries[suffix] for name, entries in env_sites.items()}
            else:
                item, operation = suffix.rsplit(':', 1)
                found = {name + ':' + suffix: len(re.findall(r'\b' + re.escape(operation) + r'\s*\(', bodies.get(item, '')))
                         for name, bodies in functions.items()}
            found = {key: value for key, value in found.items() if value}
            if sum(found.values()) != count or result.keys() & found.keys():
                raise ValueError(f'non-conserving or colliding relocation: {key}: {count} -> {found}')
            result.update(found)
        if 'sites' in data:
            data['sites'] = result
        else:
            data = result
        updates[path] = json.dumps(data, indent=2, sort_keys=True) + '\n'

    # Vendor inventory keys include the enclosing item and token kind.
    path = root / 'aida-cli-lib/src/tests/story_2539_vendor_literals.tsv'
    lines = []
    for line in path.read_text().splitlines(keepends=True):
        if line.startswith(old + '|'):
            _, item, token_count = line.rstrip('\n').split('|')
            candidates = [name for name, bodies in functions.items() if item in bodies]
            if len(candidates) > 1:
                kind, token = token_count.split('\t')[0].split(':')
                candidates = [name for name in candidates if ('"' + token + '"' if kind == 'literal' else token) in functions[name][item]]
            if len(candidates) != 1:
                raise ValueError(f'ambiguous vendor relocation: {line.strip()}: {candidates}')
            line = candidates[0] + line[len(old):]
        lines.append(line)
    updates[path] = ''.join(lines)

    # Preserve all reviewed writer policy fields, including when a shared
    # dispatcher sink moves to more than one subcommand module.
    discovered = writer.discover(root)
    path = root / 'scripts/store-writers.toml'
    lines = []
    for line in path.read_text().splitlines(keepends=True):
        if f'path = "{old}"' in line:
            item = re.search(r'item = "([^"]+)"', line).group(1)
            sink = re.search(r'sink = "([^"]+)"', line).group(1)
            candidates = [site['path'] for site in discovered.values()
                          if site['path'] in destinations and site['item'] == item and site['sink'] == sink]
            if not candidates:
                raise ValueError(f'writer site lost: {item}|{sink}')
            lines.extend(line.replace(old, name) for name in sorted(set(candidates)))
        else:
            lines.append(line)
    updates[path] = ''.join(lines)
    path = root / 'scripts/config-trust.toml'
    if path.exists():
        text = path.read_text()
        header = f'[readers."{old}"]'
        if header in text:
            section = re.search(re.escape(header) + r'\n(.*?)(?=\n\[|\Z)', text, re.S)
            body = section.group(1)
            budget = int(re.search(r'^sites = (\d+)$', body, re.M).group(1))
            counts = {}
            for name, source in sources.items():
                production, _ = writer.production(name, source)
                count = sum('config.toml"' in line and not line.lstrip().startswith('//')
                            for line in production.splitlines())
                if count:
                    counts[name] = count
            if sum(counts.values()) != budget:
                raise ValueError('config reader split did not conserve the site budget')
            replacement = ''.join(f'[readers."{name}"]\n' +
                                  re.sub(r'^sites = \d+$', f'sites = {count}', body, flags=re.M)
                                  for name, count in sorted(counts.items()))
            updates[path] = text[:section.start()] + replacement + text[section.end():]
    # Publish only after every mapping has validated.
    for path, text in updates.items():
        path.write_text(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('old')
    parser.add_argument('destinations', nargs='+')
    args = parser.parse_args()
    relocate(ROOT, args.old, args.destinations)


if __name__ == '__main__':
    main()
