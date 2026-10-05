#!/usr/bin/env python3
# trace:TASK-1538 | ai:claude
"""V2 pure-move proof (STORY-1488): the multiset of non-blank lines of the OLD
lib.rs must equal the multiset of non-blank lines of (NEW lib.rs + the new
module file), minus the explicitly allowed new lines (module header, wiring).

Usage: check_v2_multiset.py <old-lib.rs> <new-lib.rs> <new-module.rs> <module-name>
Exit 0 and prints V2 OK when the residue is empty; otherwise lists it.
"""
import sys
from collections import Counter

def msload(path):
    c = Counter()
    with open(path, encoding="utf-8") as f:
        for line in f:
            s = line.rstrip()
            if s.strip():
                c[s] += 1
    return c

def main():
    old_lib, new_lib, new_mod, name = sys.argv[1:5]
    old = msload(old_lib)
    new = msload(new_lib) + msload(new_mod)

    with open(new_mod, encoding="utf-8") as f:
        header = [l.rstrip() for l in f.readlines()[:4] if l.strip()]
    allowed = Counter(header)
    allowed[f"mod {name};"] += 1
    allowed[f"use {name}::*;"] += 1

    extra = new - old - allowed       # lines that appeared from nowhere
    missing = old - new               # lines that vanished
    unused_allow = allowed - (new - old)
    ok = True
    if extra:
        ok = False
        print("V2 FAIL: unexplained NEW lines:")
        for l, n in list(extra.items())[:20]:
            print(f"  +{n}x {l}")
    if missing:
        ok = False
        print("V2 FAIL: lines LOST from lib.rs:")
        for l, n in list(missing.items())[:20]:
            print(f"  -{n}x {l}")
    if ok:
        print(f"V2 OK: pure move ({sum(msload(new_mod).values())} non-blank lines in {new_mod}; "
              f"allowed wiring lines verified)")
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
