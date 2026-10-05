#!/usr/bin/env python3
# trace:TASK-1538 | ai:claude
"""Apply `pub(crate) ` insertions emitted by `itemtool visibility` (STORY-1488
commit A / step A). Reads the TSV on stdin, edits the file in place, bottom-up
so earlier insertions never shift later positions.

Usage: itemtool visibility <file.rs> | python3 apply_visibility.py <file.rs>
"""
import sys

def main() -> None:
    path = sys.argv[1]
    edits = []  # (line_1based, col_0based)
    for raw in sys.stdin.read().splitlines()[1:]:  # skip header row
        if not raw.strip():
            continue
        line, col, _why = raw.split("\t", 2)
        edits.append((int(line), int(col)))

    with open(path, encoding="utf-8") as f:
        lines = f.readlines()

    # Bottom-up, and right-to-left within a line.
    for line_no, col in sorted(edits, reverse=True):
        s = lines[line_no - 1]
        assert col <= len(s), f"col {col} beyond line {line_no}"
        lines[line_no - 1] = s[:col] + "pub(crate) " + s[col:]

    with open(path, "w", encoding="utf-8") as f:
        f.writelines(lines)
    print(f"applied {len(edits)} pub(crate) insertions to {path}")

if __name__ == "__main__":
    main()
