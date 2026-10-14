#!/usr/bin/env python3
"""Install or verify the repo hook links declared by the settings master."""
# trace:BUG-2014 | ai:codex
import argparse
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sync', action='store_true')
    args = parser.parse_args()
    settings = json.loads((ROOT / 'aida-core/templates/settings.json').read_text())
    paths = sorted({
        path
        for entries in settings['hooks'].values()
        for entry in entries
        for hook in entry['hooks']
        for path in re.findall(r'\.claude/hooks/[\w-]+\.sh', hook.get('command', ''))
    })
    tracked = set(subprocess.check_output(
        ['git', 'ls-files', '-z', '--', '.claude/hooks'], cwd=ROOT,
    ).decode().split('\0'))
    errors = []
    for path in paths:
        target = ROOT / path
        master = ROOT / 'aida-core/templates/hooks' / target.name
        if args.sync:
            if not master.is_file():
                errors.append(f'MISSING: hook master {master.relative_to(ROOT)}')
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            target.unlink(missing_ok=True)
            target.symlink_to('../../aida-core/templates/hooks/' + target.name)
            print(f'Linked: {path}')
        elif not target.is_file() or path not in tracked:
            errors.append(f'MISSING or UNTRACKED: {path}')
        else:
            print(f'OK: {path} (tracked hook)')
    for error in errors:
        print(error)
    return bool(errors)


if __name__ == '__main__':
    raise SystemExit(main())
