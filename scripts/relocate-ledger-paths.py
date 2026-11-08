#!/usr/bin/env python3
"""Relocate ledger paths for byte-identical moves from --base (default HEAD).

Usage: relocate-ledger-paths.py OLD NEW [--base REF]. The old path must be
absent and the destination must exactly match the committed old blob.
Stage the move before running the glob ratchet so Git can detect its rename.
Rewrites portability-allowlist.txt, exec-context-legacy.json,
crate-globs-legacy.json and ownership-liveness-legacy.json under scripts/,
plus aida-cli-lib/src/tests/fixtures/proc_identity_legacy_baseline.json and
aida-cli-lib/src/tests/story_2539_vendor_literals.tsv (path|item|token keys).
Other path-keyed inventories require separate updates after a move:
- scripts/config-trust.toml: manually rename [readers."OLD"] to [readers."NEW"],
  preserving channels and site counts; run bug_1723_config_trust_tests.
- scripts/store-writers.toml: run
  python3 scripts/check-store-writers.py --regenerate and review the diff.
- scripts/git-maintenance-baseline.txt: run
  python3 tests/test_git_maintenance_ratchet.py --write-baseline and review the diff.

CI pins the portability growth verifier from the base revision; bases predating
relocation support still refuse moved portability rows until that support merges.
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
    lines = []
    for line in text.splitlines(keepends=True):
        if line.startswith(old + '|'):
            line = new + line[len(old):]
        elif not line.startswith('#'):
            line = line.replace('\t' + old + ':', '\t' + new + ':', 1)
        lines.append(line)
    return ''.join(lines)


def assert_relocated(text, old, json_format=False):
    """Refuse stale path keys, while allowing old paths in evidence or comments."""
    if json_format:
        data = json.loads(text)
        stale = any(key == old or key.startswith(old + ':')
                    for key in data.get('sites', data))
    else:
        stale = any(line.startswith(old + '|') or '\t' + old + ':' in line for line in text.splitlines()
                    if not line.startswith('#'))
    if stale:
        raise ValueError(f'stale path remains after relocation: {old}')


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
    for name in ('scripts/portability-allowlist.txt', 'scripts/exec-context-legacy.json',
                 'scripts/crate-globs-legacy.json', 'scripts/ownership-liveness-legacy.json',
                 'aida-cli-lib/src/tests/fixtures/proc_identity_legacy_baseline.json',
                 'aida-cli-lib/src/tests/story_2539_vendor_literals.tsv'):
        path = root / name
        updates[path] = rewrite(path.read_text(), old, new, path.suffix == '.json')
        # Validate every result before publishing any ledger update.
        assert_relocated(updates[path], old, path.suffix == '.json')
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
