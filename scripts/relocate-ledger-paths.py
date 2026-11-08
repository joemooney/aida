#!/usr/bin/env python3
"""Relocate ledger paths for byte-identical moves from --base (default HEAD).

Usage: relocate-ledger-paths.py OLD NEW [--base REF]. The old path must be
absent and the destination must exactly match the committed old blob.
"""
# trace:TASK-3428 | ai:codex
import argparse
import json
from pathlib import Path, PurePosixPath
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def rewrite(text, old, new, json_format=False):
    if json_format:
        data = json.loads(text)
        sites = data.get('sites', data)
        mapped = {}
        for key, value in sites.items():
            key = new + key[len(old):] if key == old or key.startswith(old + ':') else key
            if key in mapped:
                raise ValueError(f'relocation collision: {key}')
            mapped[key] = value
        if 'sites' in data:
            data['sites'] = mapped
        else:
            data = mapped
        return json.dumps(data, indent=2, sort_keys=True) + '\n'
    return ''.join(line.replace('\t' + old + ':', '\t' + new + ':', 1) if not line.startswith('#') else line
                   for line in text.splitlines(keepends=True))


def relocate(root, old, new, base):
    for name in (old, new):
        path = PurePosixPath(name)
        if path.is_absolute() or '..' in path.parts or str(path) != name:
            raise ValueError('paths must be canonical repository-relative paths')
        if not (root / name).resolve().is_relative_to(root.resolve()):
            raise ValueError('path escapes repository')
    blob = subprocess.check_output(['git', 'show', f'{base}:{old}'], cwd=root)
    if (root / old).exists() or (root / old).is_symlink() or (root / new).is_symlink() or (root / new).read_bytes() != blob:
        raise ValueError('relocation requires an absent source and byte-identical destination')
    updates = {}
    for name in ('scripts/portability-allowlist.txt', 'scripts/exec-context-legacy.json', 'scripts/crate-globs-legacy.json'):
        path = root / name
        updates[path] = rewrite(path.read_text(), old, new, path.suffix == '.json')
    for path, text in updates.items():
        path.write_text(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('old')
    parser.add_argument('new')
    parser.add_argument('--base', default='HEAD')
    args = parser.parse_args()
    relocate(ROOT, args.old, args.new, args.base)


if __name__ == '__main__':
    main()
