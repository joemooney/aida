#!/usr/bin/env python3
"""Port one PR file's textual hunks into explicitly selected split modules.

Host-side merge aid; never changes the index or resolves Git conflict entries.
"""
# trace:STORY-2692 | ai:codex
import argparse
from pathlib import Path
import re
import shlex
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
        if new and new != old and any(
                lines[start:start + len(new)] == new
                for lines in files.values()
                for start in range(len(lines) - len(new) + 1)):
            errors.append(f'hunk {number}: replacement already present; refusing possible replay')
            continue
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


def validate_inventory(data):
    """Reject malformed policy evidence before any source writes."""
    if data.get('version') != 2:
        raise Refusal('inventory baseline must be version 2')
    for kind, required in (
            ('writer', ('id', 'path', 'item', 'sink', 'lock', 'gate', 'disposition',
                        'proposed_operation', 'evidence_tests')),
            ('operation', ('id', 'surface', 'selector', 'lock', 'gate', 'disposition',
                           'proposed_operation', 'evidence_tests'))):
        rows = data.get(kind, [])
        if not isinstance(rows, list):
            raise Refusal(f'inventory {kind} must be an array of tables')
        ids = set()
        for row in rows:
            if not isinstance(row, dict) or any(not row.get(key) for key in required):
                raise Refusal(f'incomplete inventory {kind} row')
            if any(not isinstance(row[key], str) for key in required if key != 'evidence_tests'):
                raise Refusal(f'invalid inventory {kind} field type')
            tests = row['evidence_tests']
            if not isinstance(tests, list) or any(not isinstance(t, str) or not t for t in tests):
                raise Refusal(f'invalid inventory {kind} evidence_tests')
            if row['disposition'] not in {'s2_typed', 'host_only', 'lane_private', 'read_only', 'blocked'}:
                raise Refusal(f'invalid inventory {kind} disposition')
            if kind == 'writer':
                path = Path(row['path'])
                if path.is_absolute() or '..' in path.parts:
                    raise Refusal('unsafe inventory writer path')
                if row['id'] != '|'.join((row['path'], row['item'], row['sink'])):
                    raise Refusal('inventory writer id must match site')
            elif row['disposition'] != 'blocked':
                raise Refusal('inventory operation must remain blocked')
            if row['id'] in ids:
                raise Refusal(f'duplicate inventory {kind} id: {row["id"]}')
            ids.add(row['id'])
            if row['disposition'] == 'blocked' and not row.get('blocker'):
                raise Refusal(f'inventory {kind} blocked row needs blocker')
    routes = data.get('mediated_route', [])
    if not isinstance(routes, list) or any(not isinstance(row, dict) for row in routes):
        raise Refusal('inventory mediated_route must be an array of tables')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--base', required=True, help='PR base commit')
    parser.add_argument('--head', required=True, help='PR head commit')
    parser.add_argument('--source', required=True, help='original split file, relative to repository root')
    parser.add_argument('--destination', action='append', required=True, help='candidate module; repeat for every candidate, including surviving parent')
    parser.add_argument('--destination-conflict-side', choices=['ours', 'theirs'], help='explicit index side for conflicted destinations; replaces their conflict-marked working text')
    parser.add_argument('--apply', action='store_true', help='write placements and regenerate store-writers inventory; default is preview')
    parser.add_argument('--inventory-baseline', type=Path, help='explicit reviewed version-2 inventory TOML for regeneration during a conflict')
    args = parser.parse_args()
    try:
        root = args.root.resolve(strict=True)
        baseline_args = []
        if args.inventory_baseline is not None:
            baseline = args.inventory_baseline.resolve(strict=True)
            validate_inventory(tomllib.loads(baseline.read_text()))
            baseline_args = ['--baseline', str(baseline)]
        if Path(args.source).is_absolute() or '..' in Path(args.source).parts:
            raise Refusal('source must be relative to root')
        # Resolve revisions separately so options cannot be injected as revisions.
        commits = [subprocess.check_output(['git', '-C', str(root), 'rev-parse', '--verify', '--end-of-options', rev + '^{commit}'], text=True).strip() for rev in (args.base, args.head)]
        def git(*argv):
            return subprocess.check_output(['git', '-C', str(root), *argv])
        merge = subprocess.run(['git', '-C', str(root), 'rev-parse', '--verify', 'MERGE_HEAD'], capture_output=True)
        if merge.returncode == 0:
            merge_head = merge.stdout.decode().strip()
            current_head = git('rev-parse', 'HEAD').decode().strip()
            if commits[1] not in (merge_head, current_head):
                raise Refusal('--head must be HEAD or the active merge head')
            bases = git('merge-base', '--all', current_head, merge_head).decode().splitlines()
            if bases != [commits[0]]:
                raise Refusal('--base must be the unique merge base of HEAD and MERGE_HEAD')
        elif subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', *commits]).returncode != 0:
            raise Refusal('--base must be an ancestor of --head')
        patch = subprocess.check_output(['git', '-C', str(root), 'diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--no-color', '--unified=3', *commits, '--', ':(literal)' + args.source]).decode('utf-8')
        inventory = root / 'scripts/store-writers.toml'
        if args.inventory_baseline is None and inventory.exists():
            validate_inventory(tomllib.loads(inventory.read_text()))
        paths = {destination(root, value) for value in args.destination}
        files = {path: path.read_bytes().decode('utf-8').splitlines(keepends=True) for path in paths}
        unmerged = {entry.split(b'\t', 1)[1].decode('utf-8')
                    for entry in git('ls-files', '--unmerged', '-z').split(b'\0') if entry}
        for path in paths:
            relative = path.relative_to(root).as_posix()
            if relative in unmerged:
                if args.destination_conflict_side is None:
                    raise Refusal(f'conflicted destination {relative}: select --destination-conflict-side explicitly')
                stage = '2' if args.destination_conflict_side == 'ours' else '3'
                files[path] = git('show', f':{stage}:{relative}').decode('utf-8').splitlines(keepends=True)
        changes = plan(parse_hunks(patch), files)
        for path in sorted(changes):
            print(f'{"port" if args.apply else "would port"}: {path.relative_to(root)}')
        if args.apply:
            for path, content in changes.items():
                path.write_bytes(content.encode('utf-8'))
            pending = sorted(unmerged - {'scripts/store-writers.toml'})
            if pending:
                print('Hunks written; inventory regeneration deferred until conflict entries are resolved and staged: ' + ', '.join(pending))
                print('Then run ' + shlex.join([sys.executable, str(root / 'scripts/check-store-writers.py'), '--root', str(root), '--regenerate', *baseline_args]))
                return 0
            print('Hunks written; regenerating inventory. On failure, edits remain for review.', flush=True)
            return subprocess.call([sys.executable, str(root / 'scripts/check-store-writers.py'), '--root', str(root), '--regenerate', *baseline_args])
        print('Preview only; use --apply to write and regenerate scripts/store-writers.toml.')
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'port refused: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
