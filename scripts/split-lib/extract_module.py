#!/usr/bin/env python3
# trace:TASK-1538 | ai:claude
"""Extract one named module out of aida-cli-lib/src/lib.rs (STORY-1488 slice 1,
incremental landing). Content-anchored: items are selected by kind+name against
`itemtool spans` output, never by line number, so the same manifest replays on
any base.

Usage:
  itemtool spans aida-cli-lib/src/lib.rs > /tmp/spans.tsv
  python3 extract_module.py --manifest docs/plans/2026-10-04-story-1488-incremental-manifest.toml \
      --module kernel --spans /tmp/spans.tsv --lib aida-cli-lib/src/lib.rs

Selector forms in the manifest `items` list:
  "fn name" | "struct Name" | "enum Name" | "trait Name" | "type Name"
  "const NAME" | "static NAME" | "mod name" | "union Name"
  "impl:Type" | "impl:Trait:for:Type"        (flattened, as `itemtool spans` prints)
  "macro:path@Lnnn"                          (top-level macro call containing line nnn)
A selector matches ALL its occurrences (cfg-gated twins move together).
"""
import argparse
import sys
import tomllib

HEADER_FMT = "//! {purpose}\n// trace:STORY-1488 | ai:claude\n\nuse crate::*;\n\n"

def load_spans(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        next(f)  # header
        for raw in f:
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
    if not out:
        sys.exit(f"ERROR: selector matched nothing: {selector!r}")
    return out

def extend_back(lines, start, floor):
    """Pull in contiguous plain `//` comment lines directly above the item
    (doc comments and attributes are already inside the syn span)."""
    i = start  # 1-based first line of the item
    while i - 1 > floor:
        prev = lines[i - 2].strip()
        if prev.startswith("//") and not prev.startswith("///"):
            i -= 1
        else:
            break
    return i

def insert_sorted(lines, pattern_prefix, new_line, matcher):
    """Insert new_line among existing lines that start with pattern_prefix,
    keeping alphabetical order. Returns True on success."""
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
    ap.add_argument("--spans", required=True)
    ap.add_argument("--lib", required=True)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    import re
    if not re.fullmatch(r"[a-z][a-z0-9_]*", args.module):
        sys.exit(f"ERROR: module name must be a plain snake_case identifier: {args.module!r}")
    with open(args.manifest, "rb") as f:
        manifest = tomllib.load(f)
    spec = manifest["module"][args.module]
    purpose, selectors = spec["purpose"], spec["items"]
    if "\n" in purpose or "\r" in purpose:
        sys.exit("ERROR: manifest purpose must be a single line (it becomes the //! header)")

    rows = load_spans(args.spans)
    chosen = []
    for sel in selectors:
        chosen.extend(match(sel, rows))
    # de-dup (a selector pair could overlap), sort by position
    seen, ordered = set(), []
    for r in sorted(chosen, key=lambda r: r["start"]):
        key = (r["start"], r["end"])
        if key not in seen:
            seen.add(key)
            ordered.append(r)
    for a, b in zip(ordered, ordered[1:]):
        if b["start"] <= a["end"]:
            sys.exit(f"ERROR: overlapping selections: {a} / {b}")

    with open(args.lib, encoding="utf-8") as f:
        lines = f.readlines()

    # Compute cut ranges with back-extension over plain comment runs.
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

    if args.dry_run:
        total = 0
        for (text, r) in moved_chunks:
            n = len(text.splitlines())
            total += n
            print(f"  {r['kind']} {r['name']}  L{r['start']}-{r['end']}  ({n} lines)")
        print(f"DRY RUN: {len(moved_chunks)} items, {total} lines -> {args.module}.rs")
        return

    # Cut bottom-up.
    for s, e, _ in sorted(ranges, reverse=True):
        del lines[s - 1:e]
        # collapse a doubled blank left at the seam
        if 0 < s - 1 < len(lines) and lines[s - 2].strip() == "" and lines[s - 1].strip() == "":
            del lines[s - 1]

    # Wire up: `mod x;` alphabetically, `use x::*;` in the glob block.
    # Anchors must be CRATE-ROOT lines (column 0): inline test mods contain
    # indented `use super::*;` lines that would otherwise match and put the
    # wiring inside a mod body (self-check finding 5a on PR 2425).
    def mod_matcher(l):
        if l.startswith((" ", "\t")):
            return None
        ls = l.rstrip()
        if ls.startswith("mod ") and ls.endswith(";"):
            return ls[4:-1]
        return None
    def glob_matcher(l):
        if l.startswith((" ", "\t")):
            return None
        ls = l.rstrip()
        if ls.startswith("use ") and ls.endswith("::*;"):
            return ls[4:-4]
        return None
    ok1 = insert_sorted(lines, "mod ", f"mod {args.module};\n", mod_matcher)
    ok2 = insert_sorted(lines, "use ", f"use {args.module}::*;\n", glob_matcher)
    if not (ok1 and ok2):
        sys.exit("ERROR: could not find mod/glob insertion anchors in lib.rs")

    out_path = args.lib.rsplit("/", 1)[0] + f"/{args.module}.rs"
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(HEADER_FMT.format(purpose=purpose))
        f.write("".join(text for text, _ in moved_chunks))
    with open(args.lib, "w", encoding="utf-8") as f:
        f.writelines(lines)
    moved = sum(len(t.splitlines()) for t, _ in moved_chunks)
    print(f"moved {len(moved_chunks)} items ({moved} lines) -> {out_path}")

if __name__ == "__main__":
    main()
