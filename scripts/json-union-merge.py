#!/usr/bin/env python3
"""Git merge driver `aida-json-union` for append-only JSON list files.

Usage (git config): merge.aida-json-union.driver = python3 -I <this> %O %A %B
Three-way merges objects key by key, unions string lists (removals on either
side win) and matches ratchet groups by package/target/filter. Any genuine
disagreement exits 1, so git records an ordinary conflict.
trace:BUG-2784 | ai:claude
"""
import json
import sys


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


def main(base_path, ours_path, theirs_path):
    try:
        sides = [json.loads(open(p, encoding='utf-8').read()) for p in (base_path, ours_path, theirs_path)]
        result = merge(*sides)
    except (ValueError, Conflict) as why:
        print(f'aida-json-union: conflict ({why}); leaving it to git', file=sys.stderr)
        return 1
    with open(ours_path, 'w', encoding='utf-8') as out:
        out.write(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main(*sys.argv[1:4]))
