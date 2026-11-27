#!/usr/bin/env python3
"""Git merge driver `aida-json-union` for append-only JSON list files.

Usage (git config): merge.aida-json-union.driver = python3 -I <this> %O %A %B
Three-way merges objects key by key, unions string lists (removals on either
side win) and matches ratchet groups by package/target/filter. Any genuine
disagreement, invalid JSON or other failure writes standard conflict markers
into %A (git merge-file's, else one whole-file conflict) and exits 3, so a
resolver never sees a clean-looking file. Git records exit 3 as a conflict;
AIDA's wrapper maps it to 1 and text-merges on any other non-zero exit.
trace:BUG-2784 | ai:claude
"""
import json
import subprocess
import sys

CONFLICT_EXIT = 3


class Conflict(Exception):
    pass


def ident(item):
    if isinstance(item, dict) and 'filter' in item:
        return json.dumps([item.get('package'), item.get('target'), item['filter']])
    return json.dumps(item, sort_keys=True)


MISSING = object()


def pick(base, ours, theirs):
    """Resolve a key/entry present on at most one side after the base."""
    if ours is MISSING and theirs is MISSING:
        return MISSING
    if ours is MISSING or theirs is MISSING:
        kept = theirs if ours is MISSING else ours
        if base is MISSING:
            return kept  # added on one side
        if base == kept:
            return MISSING  # removed on one side, untouched on the other
        raise Conflict('removed on one side, edited on the other')
    return merge(None if base is MISSING else base, ours, theirs)


def merge(base, ours, theirs):
    if ours == theirs:
        return ours
    if base == ours:
        return theirs
    if base == theirs:
        return ours
    if isinstance(ours, dict) and isinstance(theirs, dict):
        base = base if isinstance(base, dict) else {}
        keys = list(ours) + [k for k in theirs if k not in ours]
        out = {k: pick(base.get(k, MISSING), ours.get(k, MISSING), theirs.get(k, MISSING))
               for k in keys}
        return {k: v for k, v in out.items() if v is not MISSING}
    if isinstance(ours, list) and isinstance(theirs, list):
        base = base if isinstance(base, list) else []
        b, o, t = ({ident(x): x for x in side} for side in (base, ours, theirs))
        if len(b) != len(base) or len(o) != len(ours) or len(t) != len(theirs):
            raise Conflict('duplicate list entries')
        keys = list(o) + [k for k in t if k not in o]
        out = [pick(b.get(k, MISSING), o.get(k, MISSING), t.get(k, MISSING)) for k in keys]
        out = [x for x in out if x is not MISSING]
        strings = all(isinstance(x, str) for x in ours + theirs)
        if strings and ours == sorted(ours) and theirs == sorted(theirs):
            out.sort()
        return out
    raise Conflict(f'both sides changed {ours!r:.60}')


def markers(base_path, ours_path, theirs_path):
    """Conflict text for %A: git merge-file's hunks, else a whole-file conflict."""
    try:
        text = subprocess.run(['git', 'merge-file', '-p', '-L', 'ours', '-L', 'base', '-L', 'theirs',
                               ours_path, base_path, theirs_path],
                              capture_output=True, timeout=30)
        if 0 < text.returncode < 128 and b'\n<<<<<<< ' in b'\n' + text.stdout:
            return text.stdout
    except (OSError, subprocess.SubprocessError):
        pass
    # A clean (or failed) text merge must not pass as resolved: conflict whole files.
    ours, theirs = (open(p, 'rb').read() for p in (ours_path, theirs_path))
    end = lambda data: data if data.endswith(b'\n') or not data else data + b'\n'
    return b'<<<<<<< ours\n' + end(ours) + b'=======\n' + end(theirs) + b'>>>>>>> theirs\n'


def main(base_path, ours_path, theirs_path):
    try:
        sides = [json.loads(open(p, encoding='utf-8').read()) for p in (base_path, ours_path, theirs_path)]
        out = (json.dumps(merge(*sides), indent=2, ensure_ascii=False) + '\n').encode('utf-8')
        status = 0
    except Exception as why:  # any failure, not only a detected one, keeps both sides
        print(f'aida-json-union: conflict ({why}); writing conflict markers', file=sys.stderr)
        out, status = markers(base_path, ours_path, theirs_path), CONFLICT_EXIT
    with open(ours_path, 'wb') as result:
        result.write(out)
    return status


if __name__ == '__main__':
    sys.exit(main(*sys.argv[1:4]))
