#!/usr/bin/env python3
# trace:TASK-1538 | ai:claude
import argparse
import sys
import tomllib
import subprocess
import os

HEADER_FMT = "//! {purpose}\n// trace:STORY-1488 | ai:claude\n\nuse crate::*;\n\n"

def get_spans_for_file(path):
    # Run itemtool spans
    cmd = ["cargo", "run", "--quiet", "--manifest-path", "scripts/split-lib/itemtool/Cargo.toml", "--", "spans", path]
    res = subprocess.run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        sys.exit(f"ERROR: itemtool spans failed for {path}:\n{res.stderr}")
    rows = []
    lines = res.stdout.strip().split("\n")
    if not lines or not lines[0].startswith("start"):
        return []
    for raw in lines[1:]:
        parts = raw.rstrip("\n").split("\t")
        if len(parts) < 5:
            parts += [""] * (5 - len(parts))
        start, end, kind, name, vis = parts[:5]
        rows.append({"start": int(start), "end": int(end), "kind": kind,
                     "name": name, "vis": vis})
    return rows

def match(selector, rows):
    out = []
    if selector.startswith("impl:"):
        out = [r for r in rows if r["kind"] == "impl" and r["name"] == selector]
    elif selector.startswith("macro:"):
        if "@L" in selector:
            path, line = selector.rsplit("@L", 1)
            line = int(line)
            out = [r for r in rows if r["kind"].startswith(path)
                   and r["start"] <= line <= r["end"]]
        else:
            out = [r for r in rows if r["kind"] == selector]
    else:
        kind, _, name = selector.partition(" ")
        out = [r for r in rows if r["kind"] == kind and r["name"] == name]
    return out

def extend_back(lines, start, floor):
    i = start
    while i - 1 > floor:
        prev = lines[i - 2].strip()
        if prev.startswith("//") and not prev.startswith("///"):
            i -= 1
        else:
            break
    return i

def insert_sorted(lines, pattern_prefix, new_line, matcher):
    anchors = [(idx, m) for idx, l in enumerate(lines)
               if (m := matcher(l)) is not None]
    if not anchors:
        return False
    for idx, name in anchors:
        if name > new_line_key(new_line, pattern_prefix):
            lines.insert(idx, new_line)
            return True
    lines.insert(anchors[-1][0] + 1, new_line)
    return True

def new_line_key(line, prefix):
    return line.strip()[len(prefix):].rstrip(";\n")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--module", required=True)
    ap.add_argument("--lib", required=True)
    ap.add_argument("--parts", required=True)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    import re
    if not re.fullmatch(r"[a-z][a-z0-9_]*", args.module):
        sys.exit(f"ERROR: module name must be a plain snake_case identifier: {args.module!r}")
    with open(args.manifest, "rb") as f:
        manifest = tomllib.load(f)
    spec = manifest["module"][args.module]
    purpose, selectors = spec["purpose"], spec["items"]
    
    parts = args.parts.split(",")
    
    all_moved_chunks = []
    
    # Track which selectors have been matched at least once
    matched_selectors = {sel: False for sel in selectors}
    
    for part in parts:
        rows = get_spans_for_file(part)
        chosen = []
        for sel in selectors:
            matches = match(sel, rows)
            if matches:
                matched_selectors[sel] = True
            chosen.extend(matches)
            
        if not chosen:
            continue
            
        seen, ordered = set(), []
        for r in sorted(chosen, key=lambda r: r["start"]):
            key = (r["start"], r["end"])
            if key not in seen:
                seen.add(key)
                ordered.append(r)
        for a, b in zip(ordered, ordered[1:]):
            if b["start"] <= a["end"]:
                sys.exit(f"ERROR: overlapping selections: {a} / {b}")

        with open(part, encoding="utf-8") as f:
            lines = f.readlines()

        all_rows_sorted = sorted(rows, key=lambda r: r["start"])
        prev_end = {}
        for a, b in zip(all_rows_sorted, all_rows_sorted[1:]):
            prev_end[b["start"]] = a["end"]
        ranges = []
        for r in ordered:
            floor = prev_end.get(r["start"], 0)
            s = extend_back(lines, r["start"], floor)
            ranges.append((s, r["end"], r))

        moved_chunks = [("".join(lines[s - 1:e]), r) for s, e, r in ranges]
        all_moved_chunks.extend(moved_chunks)

        if not args.dry_run:
            for s, e, _ in sorted(ranges, reverse=True):
                del lines[s - 1:e]
                if 0 < s - 1 < len(lines) and lines[s - 2].strip() == "" and lines[s - 1].strip() == "":
                    del lines[s - 1]
            with open(part, "w", encoding="utf-8") as f:
                f.writelines(lines)

    for sel, matched in matched_selectors.items():
        if not matched:
            sys.exit(f"ERROR: selector matched nothing: {sel!r}")

    if args.dry_run:
        total = 0
        for (text, r) in all_moved_chunks:
            n = len(text.splitlines())
            total += n
            print(f"  {r['kind']} {r['name']}  L{r['start']}-{r['end']}  ({n} lines)")
        print(f"DRY RUN: {len(all_moved_chunks)} items, {total} lines -> {args.module}.rs")
        return

    # Update lib.rs
    with open(args.lib, encoding="utf-8") as f:
        lib_lines = f.readlines()

    def mod_matcher(l):
        if l.startswith((" ", "\t")): return None
        ls = l.rstrip()
        if ls.startswith("mod ") and ls.endswith(";"): return ls[4:-1]
        return None
    def glob_matcher(l):
        if l.startswith((" ", "\t")): return None
        ls = l.rstrip()
        if ls.startswith("use ") and ls.endswith("::*;"): return ls[4:-4]
        return None

    ok1 = insert_sorted(lib_lines, "mod ", f"mod {args.module};\n", mod_matcher)
    ok2 = insert_sorted(lib_lines, "use ", f"use {args.module}::*;\n", glob_matcher)
    if not (ok1 and ok2):
        sys.exit("ERROR: could not find mod/glob insertion anchors in lib.rs")

    out_path = args.lib.rsplit("/", 1)[0] + f"/{args.module}.rs"
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(HEADER_FMT.format(purpose=purpose))
        for n, (text, _) in enumerate(all_moved_chunks):
            if n > 0 and not text.startswith("\n"):
                f.write("\n")
            f.write(text)
            
    with open(args.lib, "w", encoding="utf-8") as f:
        f.writelines(lib_lines)
        
    moved = sum(len(t.splitlines()) for t, _ in all_moved_chunks)
    print(f"moved {len(all_moved_chunks)} items ({moved} lines) -> {out_path}")

if __name__ == "__main__":
    main()
