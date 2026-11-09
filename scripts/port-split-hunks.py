#!/usr/bin/env python3
"""Port one PR file's textual hunks into explicitly selected split modules.

Host-side merge aid; never changes the index or resolves Git conflict entries.
"""
# trace:STORY-2692 | ai:codex
import argparse
from pathlib import Path
import re
import subprocess
import sys
import tomllib


class Refusal(ValueError):
    pass


def parse_hunks(patch):
    """Retain exact line endings, including Git's missing-newline markers."""
    hunks = []
    old, new = None, None
    expected = None
    for line in patch.splitlines(keepends=True):
        if line.startswith('@@ '):
            match = re.fullmatch(r'@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@[^\n]*\n?', line)
            if not match:
                raise Refusal('unsupported hunk header')
            old, new = [], []
            expected = (int(match[1] or 1), int(match[2] or 1))
            hunks.append((old, new, expected))
        elif old is not None:
            if line.startswith('\\ No newline at end of file'):
                for block in last_blocks:
                    block[-1] = block[-1].removesuffix('\n')
            elif line[:1] in (' ', '-', '+'):
                last_blocks = []
                if line[0] in ' -':
                    old.append(line[1:])
                    last_blocks.append(old)
                if line[0] in ' +':
                    new.append(line[1:])
                    last_blocks.append(new)
            else:
                raise Refusal('unsupported patch content')
    for old, new, expected in hunks:
        if (len(old), len(new)) != expected:
            raise Refusal('incomplete hunk')
    if not hunks:
        raise Refusal('no textual hunks (empty, binary or mode-only change)')
    return [(old, new) for old, new, _ in hunks]


def plan(hunks, files):
    """Match against the original destination snapshot, never an earlier edit."""
    placements = {path: [] for path in files}
    errors = []
    for number, (old, new) in enumerate(hunks, 1):
        matches = [(path, start) for path, lines in files.items()
                   for start in range(len(lines) - len(old) + 1)
                   if old and lines[start:start + len(old)] == old]
        if len(matches) != 1:
            locations = ', '.join(f'{path}:{start + 1}' for path, start in matches)
            errors.append(f'hunk {number}: {len(matches)} exact matches; requires one unique placement'
                          + (f' ({locations})' if locations else ''))
            continue
        path, start = matches[0]
        end = start + len(old)
        if any(start < prior_end and prior_start < end
               for prior_start, prior_end, _ in placements[path]):
            errors.append(f'hunk {number}: overlaps another hunk in {path}')
        placements[path].append((start, end, new))
    if errors:
        raise Refusal('\n'.join(errors) + '\nNo files written. Supply missing destinations or handle manually, including required re-exports.')
    result = {}
    for path, edits in placements.items():
        lines = files[path][:]
        for start, end, new in sorted(edits, reverse=True):
            lines[start:end] = new
        if edits:
            result[path] = ''.join(lines)
    return result


def destination(root, value):
    path = root / value
    if Path(value).is_absolute() or '..' in Path(value).parts:
        raise Refusal(f'destination must be relative to root: {value}')
    if any(part.is_symlink() for part in (path, *path.parents)):
        raise Refusal(f'symlink destination refused: {value}')
    if not path.is_file():
        raise Refusal(f'destination is not an existing regular file: {value}')
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--base', required=True, help='PR base commit')
    parser.add_argument('--head', required=True, help='PR head commit')
    parser.add_argument('--source', required=True, help='original split file, relative to repository root')
    parser.add_argument('--destination', action='append', required=True, help='candidate module; repeat for every candidate, including surviving parent')
    parser.add_argument('--apply', action='store_true', help='write placements and regenerate store-writers inventory; default is preview')
    parser.add_argument('--inventory-baseline', type=Path, help='explicit reviewed version-2 inventory TOML for regeneration during a conflict')
    args = parser.parse_args()
    try:
        root = args.root.resolve(strict=True)
        baseline_args = []
        if args.inventory_baseline is not None:
            baseline = args.inventory_baseline.resolve(strict=True)
            if tomllib.loads(baseline.read_text()).get('version') != 2:
                raise Refusal('inventory baseline must be version 2')
            baseline_args = ['--baseline', str(baseline)]
        if Path(args.source).is_absolute() or '..' in Path(args.source).parts:
            raise Refusal('source must be relative to root')
        # Resolve revisions separately so options cannot be injected as revisions.
        commits = [subprocess.check_output(['git', '-C', str(root), 'rev-parse', '--verify', '--end-of-options', rev + '^{commit}'], text=True).strip() for rev in (args.base, args.head)]
        patch = subprocess.check_output(['git', '-C', str(root), 'diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--no-color', '--unified=3', *commits, '--', ':(literal)' + args.source]).decode('utf-8')
        paths = {destination(root, value) for value in args.destination}
        files = {path: path.read_bytes().decode('utf-8').splitlines(keepends=True) for path in paths}
        changes = plan(parse_hunks(patch), files)
        for path in sorted(changes):
            print(f'{"port" if args.apply else "would port"}: {path.relative_to(root)}')
        if args.apply:
            for path, content in changes.items():
                path.write_bytes(content.encode('utf-8'))
            print('Hunks written; regenerating inventory. On failure, edits remain for review.', flush=True)
            return subprocess.call([sys.executable, str(root / 'scripts/check-store-writers.py'), '--root', str(root), '--regenerate', *baseline_args])
        print('Preview only; use --apply to write and regenerate scripts/store-writers.toml.')
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'port refused: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
